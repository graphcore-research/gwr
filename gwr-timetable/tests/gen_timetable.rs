// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::syntax::{ExprSpec, IndexSpec, ReductionOp};
use gwr_models::processing_element::tensors::HasShape;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{
    NodeSection, ProgramDefinition, ProgramSection, TensorConfigSection, TensorViewSection,
    TensorViewSections, TimetableFile,
};

#[test]
fn generator_ignores_unreachable_addresses_in_the_first_pe_map() {
    let fixture = GeneratorFixture::new(
        r"
memory_maps:
  - { name: unreachable, devices: [hidden] }
  - { name: mapped, devices: [mem] }
operation_classes:
  add: { operations: [add], arity: 2 }
processing_elements:
  - name: pe_unmapped
    memory_map: unreachable
    config: &pe_config
      compute:
        engines:
          vector:
            capabilities: [{ class: add, ops_per_tick: 256 }]
  - name: pe0
    memory_map: mapped
    config: *pe_config
memories:
  - { name: hidden, kind: hbm, base_address: 0, config: { capacity_bytes: 4096 } }
  - { name: mem, kind: hbm, base_address: 4096, config: { capacity_bytes: 4096 } }
  - { name: unmapped, kind: hbm, base_address: 8192, config: { capacity_bytes: 4096 } }
fabrics:
  - { name: isolated, kind: functional, columns: 2, rows: 1, config: { routing: column-first } }
connections:
  - connect: [pe.pe_unmapped, mem.unmapped]
  - connect: [mem.hidden, 'fabric.isolated@(0,0)']
  - connect: ['fabric.isolated@(1,0)', 'null']
  - connect: [pe.pe0, mem.mem]
",
    );
    let file = fixture.generate(SMALL_ADD_ARGS);
    assert_eq!(compute_program(&file).0, "pe0");
    assert_eq!(tensor_addresses(&file), [4096, 4128, 4160]);
    fixture.run_to_completion(file);
}

#[test]
fn generator_checks_the_last_reachable_allocation_byte() {
    for (capacity, succeeds) in [(79, false), (80, true)] {
        let yaml = PLATFORM_YAML
            .replace(
                "capacity_bytes: 0x1000_0000",
                &format!("capacity_bytes: {capacity}"),
            )
            .replace(
                "      compute:",
                "      lsu_access_bytes: 1\n      compute:",
            );
        let fixture = GeneratorFixture::new(&yaml);
        if succeeds {
            let file = fixture.generate(SMALL_ADD_ARGS);
            assert_eq!(tensor_addresses(&file), [0, 32, 64]);
            fixture.run_to_completion(file);
        } else {
            fs::write(&fixture.timetable_path, b"existing output").unwrap();
            assert_generation_failed(&fixture.run_generator(SMALL_ADD_ARGS), "placement");
            assert_eq!(
                fs::read(&fixture.timetable_path).unwrap(),
                b"existing output"
            );
        }
    }
}

#[test]
fn generator_relocates_a_shared_rhs_into_memory_mapped_by_both_pes() {
    let fixture = GeneratorFixture::new(
        r"
memory_maps:
  - { name: first, devices: [a, shared] }
  - { name: second, devices: [b, shared] }
operation_classes:
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
processing_elements:
  - name: pe0
    memory_map: first
    config: &matrix_config
      compute:
        engines:
          matrix:
            capabilities: [{ class: matrix, ops_per_tick: 256 }]
  - name: pe1
    memory_map: second
    config: *matrix_config
fabrics:
  - { name: mesh, kind: functional, columns: 5, rows: 1, config: { routing: column-first } }
memories:
  - { name: a, kind: hbm, base_address: 0, config: { capacity_bytes: 4096 } }
  - { name: b, kind: hbm, base_address: 4096, config: { capacity_bytes: 4096 } }
  - { name: shared, kind: hbm, base_address: 8192, config: { capacity_bytes: 4096 } }
connections:
  - connect: [pe.pe0, 'fabric.mesh@(0,0)']
  - connect: [pe.pe1, 'fabric.mesh@(1,0)']
  - connect: [mem.a, 'fabric.mesh@(2,0)']
  - connect: [mem.b, 'fabric.mesh@(3,0)']
  - connect: [mem.shared, 'fabric.mesh@(4,0)']
",
    );
    let file = fixture.generate(&[
        ("--depth", "1"),
        ("--max-compute-nodes", "2"),
        ("--partition-size", "2"),
        ("--weight-add", "0"),
        ("--weight-gemm", "1"),
        ("--weight-reduce-max", "0"),
        ("--init-rank-min", "2"),
        ("--init-rank-max", "2"),
        ("--init-dim-min", "4"),
        ("--init-dim-max", "4"),
        ("--expand-ratio", "1"),
    ]);
    let graph = file.clone().into_graph().unwrap();
    let mut tasks = graph
        .nodes()
        .iter()
        .enumerate()
        .filter(|(_, node)| node.is_compute());
    let (first, pe0) = tasks.next().unwrap();
    let (second, pe1) = tasks.next().unwrap();
    assert!(tasks.next().is_none());
    assert_eq!([pe0.pe(), pe1.pe()], [Some("pe0"), Some("pe1")]);

    let first_views = graph.compute_views(first).unwrap();
    let second_views = graph.compute_views(second).unwrap();
    let first_rhs = first_views.inputs()[1].as_ref().unwrap();
    let second_rhs = second_views.inputs()[1].as_ref().unwrap();
    assert_eq!(first_rhs.tensor().id(), second_rhs.tensor().id());
    for rhs in [first_rhs, second_rhs] {
        assert_eq!(rhs.shape().dims(), &[4, 4]);
        assert_eq!(rhs.offsets(), &[0, 0]);
        assert!((8192..=12_224).contains(&rhs.tensor().addr()));
    }
    fixture.run_to_completion(file);
}

#[test]
fn generator_selects_wide_matrix_accumulation_and_executes_conversion() {
    let fixture = GeneratorFixture::new(BF16_MATRIX_PLATFORM);
    let file = fixture.generate(BF16_MATRIX_ARGS);
    assert_matrix_mode(&file, "pe0", "tensor", DataType::Fp32);
    let platform = fixture.run_to_completion(file);
    let ops = platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(ops.by_operation.get("matrix_mac"), Some(&8));
    assert_eq!(ops.by_operation.get("convert"), Some(&4));
}

#[test]
fn generator_rejects_matrix_mode_without_output_conversion() {
    let yaml = BF16_MATRIX_PLATFORM.replace("input_dtypes: [[fp32]]", "input_dtypes: [[fp16]]");
    let fixture = GeneratorFixture::new(&yaml);
    assert_generation_failed(
        &fixture.run_generator(BF16_MATRIX_ARGS),
        "no supported compute templates",
    );
    assert!(!fixture.timetable_path.exists());
}

#[test]
fn generator_tries_other_accumulators_when_preferred_accumulator_cannot_fit() {
    let yaml = BF16_MATRIX_PLATFORM.replace(
        "          matrix:",
        "          narrow:
            matrix_accumulator_capacity_bytes: 1
            capabilities:
              - class: matrix
                input_dtypes: [[bf16, bf16]]
                execution: { instruction: narrow, accumulator: bf16 }
                ops_per_tick: 128
          matrix:",
    );
    let fixture = GeneratorFixture::new(&yaml);
    let file = fixture.generate(BF16_MATRIX_ARGS);
    assert_matrix_mode(&file, "pe0", "tensor", DataType::Fp32);
    fixture.run_to_completion(file);
}

#[test]
fn generator_prefers_destination_accumulation_when_supported() {
    let yaml = BF16_MATRIX_PLATFORM.replace(
        "          vector:",
        "          narrow:
            capabilities:
              - class: matrix
                input_dtypes: [[bf16, bf16]]
                execution: { instruction: narrow, accumulator: bf16 }
                ops_per_tick: 128
          vector:",
    );
    let fixture = GeneratorFixture::new(&yaml);
    let file = fixture.generate(BF16_MATRIX_ARGS);
    assert_matrix_mode(&file, "pe0", "narrow", DataType::Bf16);
    let platform = fixture.run_to_completion(file);
    assert!(
        !platform
            .pe("pe0")
            .unwrap()
            .machine_ops()
            .by_operation
            .contains_key("convert")
    );
}

#[test]
fn generator_rebuilds_the_matrix_program_for_a_pe_that_can_fit_it() {
    let yaml = BF16_MATRIX_PLATFORM
        .replace(
            "  - name: pe0",
            "  - name: pe_small
    memory_map: mm
    config:
      sram_bytes: 1
      compute:
        engines:
          matrix:
            capabilities:
              - class: matrix
                input_dtypes: [[bf16, bf16]]
                execution: { instruction: small, accumulator: fp16 }
                ops_per_tick: 128
          vector:
            capabilities: [{ class: conversion, input_dtypes: [[fp16]], ops_per_tick: 32 }]
  - name: pe0",
        )
        .replace(
            "connections: [{ connect: [pe.pe0, mem.mem] }]",
            "fabrics: [{ name: mesh, kind: functional, columns: 3, rows: 1, config: { routing: column-first } }]
connections:
  - { connect: [pe.pe_small, 'fabric.mesh@(0,0)'] }
  - { connect: [pe.pe0, 'fabric.mesh@(1,0)'] }
  - { connect: [mem.mem, 'fabric.mesh@(2,0)'] }",
        );
    let fixture = GeneratorFixture::new(&yaml);
    let file = fixture.generate(BF16_MATRIX_ARGS);
    assert_matrix_mode(&file, "pe0", "tensor", DataType::Fp32);
    fixture.run_to_completion(file);
}

fn positional_views(views: &Option<TensorViewSections>) -> &[Option<TensorViewSection>] {
    let Some(TensorViewSections::Positional(views)) = views else {
        panic!("expected positional views in the generated timetable");
    };
    views
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep this generated graph fixture and all of its platform placement and output-view assertions together."
)]
fn generator_partitions_add_across_pes_with_disjoint_views() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let platform_path = temp_dir.path().join("platform.yaml");
    let timetable_path = temp_dir.path().join("timetable.yaml");
    let multi_pe = PLATFORM_YAML.replace(
        "memories:\n",
        "  - name: pe1\n    memory_map: default\n    config:\n      compute:\n        engines:\n          vector:\n            capabilities: [{ class: add, ops_per_tick: 256 }]\n  - name: pe2\n    memory_map: default\n    config:\n      compute:\n        engines:\n          vector:\n            capabilities: [{ class: add, ops_per_tick: 256 }]\n\nfabrics:\n  - name: fabric0\n    kind: functional\n    columns: 2\n    rows: 2\n    config: { routing: column-first }\n\nmemories:\n",
    ).replace(
        "connections:\n  - connect: [pe.pe0, mem.hbm0]",
        "connections:\n  - connect: [pe.pe0, 'fabric.fabric0@(0,0)']\n  - connect: [pe.pe1, 'fabric.fabric0@(0,1)']\n  - connect: [pe.pe2, 'fabric.fabric0@(1,0)']\n  - connect: [mem.hbm0, 'fabric.fabric0@(1,1)']",
    );
    fs::write(
        temp_dir.path().join("catalog.yaml"),
        "operation_classes:\n  extra: { operations: [extra], arity: 1 }\n",
    )
    .unwrap();
    fs::write(
        &platform_path,
        format!("includes: [catalog.yaml]\n{multi_pe}"),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
        .args(["--platform", platform_path.to_str().unwrap()])
        .args(["--out", timetable_path.to_str().unwrap()])
        .args([
            "--depth",
            "1",
            "--max-compute-nodes",
            "4",
            "--partition-size",
            "2",
        ])
        .args([
            "--weight-add",
            "1",
            "--weight-gemm",
            "0",
            "--weight-reduce-max",
            "0",
        ])
        .args(["--init-rank-min", "2", "--init-rank-max", "2"])
        .args(["--init-dim-min", "8", "--init-dim-max", "8"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = TimetableFile::from_file(&timetable_path).unwrap();
    file.validate().unwrap();
    assert_eq!(file.programs.len(), 1);
    let mut regions = Vec::new();
    let mut pes = HashSet::new();
    let mut pe_sequence = Vec::new();
    for node in &file.nodes {
        if let NodeSection::Compute {
            pe,
            program,
            input_views,
            output_views,
            tiling,
            ..
        } = node
        {
            assert!(matches!(program, ProgramSection::Named(_)));
            pes.insert(pe.as_deref().unwrap().to_owned());
            pe_sequence.push(pe.as_deref().unwrap().to_owned());
            let input_views = positional_views(input_views);
            let output_view = positional_views(output_views)[0].as_ref().unwrap();
            assert_eq!(
                input_views[0].as_ref().unwrap().offsets[0],
                output_view.offsets[0]
            );
            assert_eq!(
                input_views[0].as_ref().unwrap().shape[0],
                output_view.shape[0]
            );
            assert_eq!(
                input_views[1].as_ref().unwrap().offsets[0],
                output_view.offsets[0]
            );
            assert!(tiling.is_some());
            regions.push((output_view.offsets[0], output_view.shape[0]));
        }
    }
    regions.sort_unstable();
    assert_eq!(regions, vec![(0, 2), (2, 2), (4, 2), (6, 2)]);
    assert_eq!(
        pes,
        HashSet::from(["pe0".into(), "pe1".into(), "pe2".into()])
    );
    assert_eq!(pe_sequence, ["pe0", "pe1", "pe2", "pe0"]);

    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_file(&engine, &clock, &platform_path).unwrap());
    let timetable = Rc::new(
        Timetable::new(&engine.top().clone(), file.into_graph().unwrap(), &platform).unwrap(),
    );
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
}

#[test]
fn generator_partitions_gemm_without_slicing_shared_rhs() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let platform_path = temp_dir.path().join("platform.yaml");
    let timetable_path = temp_dir.path().join("timetable.yaml");
    fs::write(&platform_path, PLATFORM_YAML).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
        .args(["--platform", platform_path.to_str().unwrap()])
        .args(["--out", timetable_path.to_str().unwrap()])
        .args([
            "--depth",
            "1",
            "--max-compute-nodes",
            "4",
            "--partition-size",
            "2",
        ])
        .args([
            "--weight-add",
            "0",
            "--weight-gemm",
            "1",
            "--weight-reduce-max",
            "0",
        ])
        .args(["--expand-ratio", "0.5"])
        .args(["--init-rank-min", "2", "--init-rank-max", "2"])
        .args(["--init-dim-min", "8", "--init-dim-max", "8"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = TimetableFile::from_file(&timetable_path).unwrap();
    file.validate().unwrap();
    let mut regions = Vec::new();
    for node in &file.nodes {
        if let NodeSection::Compute {
            input_views,
            output_views,
            ..
        } = node
        {
            let inputs = positional_views(input_views);
            let output = positional_views(output_views)[0].as_ref().unwrap();
            let lhs = inputs[0].as_ref().unwrap();
            let rhs = inputs[1].as_ref().unwrap();
            assert_eq!(
                (lhs.offsets[0], lhs.shape[0]),
                (output.offsets[0], output.shape[0])
            );
            assert_eq!(rhs.offsets, vec![0, 0]);
            regions.push((output.offsets[0], output.shape[0]));
        }
    }
    regions.sort_unstable();
    assert_eq!(regions, vec![(0, 2), (2, 2), (4, 2), (6, 2)]);

    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, PLATFORM_YAML).unwrap());
    let timetable = Rc::new(
        Timetable::new(&engine.top().clone(), file.into_graph().unwrap(), &platform).unwrap(),
    );
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
}

#[test]
fn generator_tiles_gemm_to_fit_pe_sram() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let platform_path = temp_dir.path().join("platform.yaml");
    let timetable_path = temp_dir.path().join("timetable.yaml");
    let platform_yaml = PLATFORM_YAML.replace(
        "    config:\n      compute:",
        "    config:\n      sram_bytes: 256\n      compute:",
    );
    fs::write(&platform_path, &platform_yaml).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
        .args(["--platform", platform_path.to_str().unwrap()])
        .args(["--out", timetable_path.to_str().unwrap()])
        .args(["--depth", "1", "--max-compute-nodes", "1"])
        .args([
            "--weight-add",
            "0",
            "--weight-gemm",
            "1",
            "--weight-reduce-max",
            "0",
        ])
        .args(["--expand-ratio", "1", "--partition-size", "8"])
        .args(["--init-rank-min", "2", "--init-rank-max", "2"])
        .args(["--init-dim-min", "8", "--init-dim-max", "8"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = TimetableFile::from_file(&timetable_path).unwrap();
    let tiling = file
        .nodes
        .iter()
        .find_map(|node| match node {
            NodeSection::Compute { tiling, .. } => tiling.as_ref(),
            _ => None,
        })
        .unwrap();
    assert!(tiling.sizes.contains_key("k"));
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml).unwrap());
    let timetable = Rc::new(
        Timetable::new(&engine.top().clone(), file.into_graph().unwrap(), &platform).unwrap(),
    );
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
}

#[test]
fn generator_rejects_a_tile_that_cannot_fit_even_at_unit_size() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let platform_path = temp_dir.path().join("platform.yaml");
    let timetable_path = temp_dir.path().join("timetable.yaml");
    fs::write(
        &platform_path,
        PLATFORM_YAML.replace(
            "    config:\n      compute:",
            "    config:\n      sram_bytes: 8\n      compute:",
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
        .args(["--platform", platform_path.to_str().unwrap()])
        .args(["--out", timetable_path.to_str().unwrap()])
        .args(["--depth", "1", "--max-compute-nodes", "1"])
        .args([
            "--weight-add",
            "1",
            "--weight-gemm",
            "0",
            "--weight-reduce-max",
            "0",
        ])
        .args(["--init-rank-min", "2", "--init-rank-max", "2"])
        .args(["--init-dim-min", "8", "--init-dim-max", "8"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("minimum tile requires"));
    assert!(!timetable_path.exists());
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep this generated graph fixture and all of its platform placement and output-view assertions together."
)]
fn generator_places_only_supported_operations_on_heterogeneous_pes() {
    let platform_yaml = "
memory_maps:
  - name: default
    devices: [hbm0]
operation_classes:
  add: { operations: [add], arity: 2 }
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
processing_elements:
  - name: pe_add
    memory_map: default
    config:
      compute:
        engines:
          alu:
            capabilities: [{ class: add, ops_per_tick: 16 }]
  - name: pe_matrix
    memory_map: default
    config:
      compute:
        engines:
          matrix:
            capabilities: [{ class: matrix, ops_per_tick: 256 }]
fabrics:
  - name: fabric0
    kind: functional
    columns: 3
    rows: 1
    config: { routing: column-first }
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config: { capacity_bytes: 1GiB }
connections:
  - connect: [pe.pe_add, 'fabric.fabric0@(0,0)']
  - connect: [pe.pe_matrix, 'fabric.fabric0@(1,0)']
  - connect: [mem.hbm0, 'fabric.fabric0@(2,0)']
";
    let temp_dir = tempfile::TempDir::new().unwrap();
    let platform_path = temp_dir.path().join("platform.yaml");
    fs::write(&platform_path, platform_yaml).unwrap();
    for (name, add_weight, gemm_weight, expected_pe) in
        [("add", "1", "0", "pe_add"), ("gemm", "0", "1", "pe_matrix")]
    {
        let timetable_path = temp_dir.path().join(format!("{name}.yaml"));
        let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
            .args(["--platform", platform_path.to_str().unwrap()])
            .args(["--out", timetable_path.to_str().unwrap()])
            .args([
                "--depth",
                "1",
                "--max-compute-nodes",
                "4",
                "--partition-size",
                "2",
            ])
            .args([
                "--weight-add",
                add_weight,
                "--weight-gemm",
                gemm_weight,
                "--weight-reduce-max",
                "0",
            ])
            .args(["--expand-ratio", "1"])
            .args(["--init-rank-min", "2", "--init-rank-max", "2"])
            .args(["--init-dim-min", "8", "--init-dim-max", "8"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let file = TimetableFile::from_file(&timetable_path).unwrap();
        let mut count = 0;
        for node in &file.nodes {
            if let NodeSection::Compute { pe, program, .. } = node {
                count += 1;
                assert_eq!(pe.as_deref(), Some(expected_pe));
                let program = match program {
                    ProgramSection::Named(name) => &file.programs[name],
                    ProgramSection::Inline(program) => program,
                };
                if name == "gemm" {
                    assert_eq!(program.inputs.len(), 2, "bias needs add support");
                    assert!(matches!(
                        program.assignments[0].expr,
                        ExprSpec::MatrixMac { .. }
                    ));
                }
            }
        }
        assert_eq!(count, 4);
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let platform = Rc::new(Platform::from_file(&engine, &clock, &platform_path).unwrap());
        let timetable = Rc::new(
            Timetable::new(&engine.top().clone(), file.into_graph().unwrap(), &platform).unwrap(),
        );
        let dispatcher: Rc<dyn Dispatch> = timetable.clone();
        platform.attach_dispatcher(&dispatcher);
        engine.run().unwrap();
        timetable.check_tasks_complete().unwrap();
    }

    let default_path = temp_dir.path().join("default_partition.yaml");
    let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
        .args(["--platform", platform_path.to_str().unwrap()])
        .args(["--out", default_path.to_str().unwrap()])
        .args(["--depth", "1", "--max-compute-nodes", "4"])
        .args([
            "--weight-add",
            "0",
            "--weight-gemm",
            "1",
            "--weight-reduce-max",
            "0",
        ])
        .args(["--init-rank-min", "2", "--init-rank-max", "2"])
        .args(["--init-dim-min", "8", "--init-dim-max", "8"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = TimetableFile::from_file(&default_path).unwrap();
    let compute_count = file
        .nodes
        .iter()
        .filter(|node| matches!(node, NodeSection::Compute { .. }))
        .count();
    assert_eq!(
        compute_count, 1,
        "default partition count uses eligible PEs"
    );
}

#[test]
fn generator_rejects_templates_with_no_supported_pe_or_dtype() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let platform_path = temp_dir.path().join("platform.yaml");
    let timetable_path = temp_dir.path().join("timetable.yaml");
    fs::write(
        &platform_path,
        "memory_maps: [{ name: default, devices: [hbm0] }]\noperation_classes:\n  add: { operations: [add], arity: 2 }\nprocessing_elements:\n  - name: pe0\n    memory_map: default\n    config:\n      compute:\n        engines:\n          alu:\n            capabilities:\n              - { class: add, input_dtypes: [[fp16, fp16]], ops_per_tick: 16 }\nmemories:\n  - name: hbm0\n    kind: hbm\n    base_address: 0\n    config: { capacity_bytes: 1GiB }\nconnections: [{ connect: [pe.pe0, mem.hbm0] }]\n",
    )
    .unwrap();
    for (add, gemm, dtype) in [("0", "1", "fp16"), ("1", "0", "fp32")] {
        let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
            .args(["--platform", platform_path.to_str().unwrap()])
            .args(["--out", timetable_path.to_str().unwrap()])
            .args([
                "--weight-add",
                add,
                "--weight-gemm",
                gemm,
                "--weight-reduce-max",
                "0",
            ])
            .args(["--init-dtype", dtype])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("no supported compute templates"));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
        .args(["--platform", platform_path.to_str().unwrap()])
        .args(["--out", timetable_path.to_str().unwrap()])
        .args([
            "--weight-add",
            "1",
            "--weight-gemm",
            "1",
            "--weight-reduce-max",
            "0",
        ])
        .args(["--init-dtype", "fp16"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = TimetableFile::from_file(&timetable_path).unwrap();
    let compute_nodes = file
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeSection::Compute { program, .. } = node else {
                return None;
            };
            Some(match program {
                ProgramSection::Named(name) => &file.programs[name],
                ProgramSection::Inline(program) => program,
            })
        })
        .collect::<Vec<_>>();
    assert!(!compute_nodes.is_empty());
    assert!(compute_nodes.iter().all(|program| matches!(program.assignments[0].expr, ExprSpec::Call { ref op, .. } if op == "add")));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep this generated graph fixture and all of its platform placement and output-view assertions together."
)]
fn generator_emits_multi_output_reduce_max() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let platform_path = temp_dir.path().join("platform.yaml");
    let timetable_path = temp_dir.path().join("timetable.yaml");
    let platform_yaml = PLATFORM_YAML.replace(
        "    config:\n      compute:",
        "    config:\n      sram_bytes: 256\n      compute:",
    );
    fs::write(&platform_path, &platform_yaml).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
        .arg("--platform")
        .arg(&platform_path)
        .arg("--out")
        .arg(&timetable_path)
        .arg("--depth")
        .arg("6")
        .arg("--max-compute-nodes")
        .arg("8")
        .arg("--partition-size")
        .arg("2")
        .arg("--expand-ratio")
        .arg("1.0")
        .arg("--weight-add")
        .arg("0.0")
        .arg("--weight-gemm")
        .arg("0.0")
        .arg("--weight-reduce-max")
        .arg("1.0")
        .arg("--seed")
        .arg("1592612472")
        .arg("--init-rank-min")
        .arg("4")
        .arg("--init-rank-max")
        .arg("4")
        .arg("--init-dim-min")
        .arg("4")
        .arg("--init-dim-max")
        .arg("4")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gen-timetable failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let timetable_file = TimetableFile::from_file(&timetable_path).unwrap();

    let mut saw_reduction = false;
    let mut saw_reduction_indices = false;

    for node in &timetable_file.nodes {
        let NodeSection::Compute {
            id,
            program,
            output_views,
            ..
        } = node
        else {
            continue;
        };
        let program = match program {
            ProgramSection::Named(name) => &timetable_file.programs[name],
            ProgramSection::Inline(program) => program,
        };

        match &program.assignments[0].expr {
            ExprSpec::Call { .. } | ExprSpec::MatrixMac { .. } => {}
            ExprSpec::Reduce {
                op: ReductionOp::Min | ReductionOp::Max,
                over,
                value,
            } => {
                saw_reduction = true;
                assert_eq!(over, &["k"]);
                assert!(
                    matches!(&**value, ExprSpec::Read { indices, .. } if matches!(indices.last(), Some(IndexSpec::Axis(axis)) if axis == "k"))
                );
                let input_edge = timetable_file
                    .edges
                    .iter()
                    .find(|edge| edge.to == format!("{id}.0"))
                    .unwrap_or_else(|| panic!("missing input edge for {id}"));
                let input = tensor_config(&timetable_file, &input_edge.from);
                let output_views = positional_views(output_views);
                assert!((1..=2).contains(&output_views.len()));
                assert_eq!(program.outputs.len(), output_views.len());

                let values_edge = timetable_file
                    .edges
                    .iter()
                    .find(|edge| edge.from == format!("{id}.0"))
                    .unwrap_or_else(|| panic!("missing values edge for {id}"));
                let values = tensor_config(&timetable_file, &values_edge.to);
                assert_eq!(values.shape, input.shape[..input.shape.len() - 1]);
                if output_views.len() == 2 {
                    saw_reduction_indices = true;
                    let indices_edge = timetable_file
                        .edges
                        .iter()
                        .find(|edge| edge.from == format!("{id}.1"))
                        .unwrap_or_else(|| panic!("missing indices edge for {id}"));
                    let indices = tensor_config(&timetable_file, &indices_edge.to);
                    assert_eq!(indices.dtype, DataType::Int64);
                    assert_eq!(indices.shape, values.shape);
                }
            }
            other => panic!("unexpected generated expression {other:?}"),
        }
    }

    assert!(
        saw_reduction,
        "generated timetable did not contain max reduction"
    );
    assert!(
        saw_reduction_indices,
        "generated timetable did not contain max-reduction indices output"
    );

    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml).unwrap());
    let timetable = Rc::new(
        Timetable::new(
            &engine.top().clone(),
            timetable_file.into_graph().unwrap(),
            &platform,
        )
        .unwrap(),
    );
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
}

fn tensor_config<'a>(timetable_file: &'a TimetableFile, id: &str) -> &'a TensorConfigSection {
    timetable_file
        .nodes
        .iter()
        .find_map(|node| match node {
            NodeSection::Tensor {
                id: tensor_id,
                config,
            } if tensor_id == id => Some(config),
            _ => None,
        })
        .unwrap_or_else(|| panic!("tensor {id} not found"))
}

#[test]
fn generator_emits_valid_reduction_program() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let platform_path = temp_dir.path().join("platform.yaml");
    let timetable_path = temp_dir.path().join("timetable.yaml");
    fs::write(&platform_path, PLATFORM_YAML).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
        .args(["--platform", platform_path.to_str().unwrap()])
        .args(["--out", timetable_path.to_str().unwrap()])
        .args(["--max-compute-nodes", "1"])
        .args(["--expand-ratio", "2.0"])
        .args(["--weight-add", "0"])
        .args(["--weight-gemm", "0"])
        .args(["--weight-reduce-max", "1"])
        .args(["--init-rank-min", "4"])
        .args(["--init-rank-max", "4"])
        .args(["--init-dim-min", "2"])
        .args(["--init-dim-max", "2"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    TimetableFile::from_file(&timetable_path)
        .unwrap()
        .validate()
        .unwrap();
}

#[test]
fn generator_emits_valid_add_and_matrix_programs() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let platform_path = temp_dir.path().join("platform.yaml");
    fs::write(&platform_path, PLATFORM_YAML).unwrap();
    for (name, add_weight, gemm_weight) in [("add", "1", "0"), ("gemm", "0", "1")] {
        let timetable_path = temp_dir.path().join(format!("{name}.yaml"));
        let output = Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
            .args(["--platform", platform_path.to_str().unwrap()])
            .args(["--out", timetable_path.to_str().unwrap()])
            .args(["--max-compute-nodes", "3"])
            .args(["--weight-add", add_weight])
            .args(["--weight-gemm", gemm_weight])
            .args(["--weight-reduce-max", "0"])
            .args(["--init-rank-min", "2"])
            .args(["--init-rank-max", "2"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let file = TimetableFile::from_file(&timetable_path).unwrap();
        file.validate().unwrap();
        for node in &file.nodes {
            let NodeSection::Compute {
                pipeline: None,
                program: gwr_timetable::timetable_file::ProgramSection::Inline(program),
                ..
            } = node
            else {
                continue;
            };
            assert!(match (&program.assignments[0].expr, name) {
                (ExprSpec::Call { op, .. }, "add") => op == "add",
                (ExprSpec::MatrixMac { .. }, "gemm") => true,
                _ => false,
            });
        }
    }
}

const PLATFORM_YAML: &str = "
memory_maps:
  - name: default
    devices:
      - hbm0

operation_classes:
  add: { operations: [add], arity: 2 }
  mul: { operations: [mul], arity: 2 }
  compare: { operations: [compare], arity: 2 }
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }

processing_elements:
  - name: pe0
    memory_map: default
    config:
      compute:
        engines:
          vector:
            capabilities:
              - { class: add, ops_per_tick: 256 }
              - { class: mul, ops_per_tick: 256 }
              - { class: compare, ops_per_tick: 256 }
          matrix:
            capabilities: [{ class: matrix, ops_per_tick: 1024 }]

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
      capacity_bytes: 0x1000_0000
connections:
  - connect: [pe.pe0, mem.hbm0]
";

const BF16_MATRIX_PLATFORM: &str = "
memory_maps: [{ name: mm, devices: [mem] }]
operation_classes:
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
  conversion: { operations: [convert], arity: 1 }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: 1MiB
      compute:
        engines:
          matrix:
            capabilities:
              - class: matrix
                input_dtypes: [[bf16, bf16]]
                execution: { instruction: tensor, accumulator: fp32 }
                ops_per_tick: 128
          vector:
            capabilities:
              - { class: conversion, input_dtypes: [[fp32]], ops_per_tick: 32 }
memories:
  - { name: mem, kind: hbm, base_address: 0, config: { capacity_bytes: 1MiB } }
connections: [{ connect: [pe.pe0, mem.mem] }]
";

const SMALL_ADD_ARGS: &[(&str, &str)] = &[
    ("--depth", "1"),
    ("--max-compute-nodes", "1"),
    ("--weight-add", "1"),
    ("--weight-gemm", "0"),
    ("--weight-reduce-max", "0"),
    ("--init-rank-min", "2"),
    ("--init-rank-max", "2"),
    ("--init-dim-min", "2"),
    ("--init-dim-max", "2"),
    ("--tensor-align-bytes", "32"),
];

const BF16_MATRIX_ARGS: &[(&str, &str)] = &[
    ("--init-dtype", "bf16"),
    ("--depth", "1"),
    ("--max-compute-nodes", "1"),
    ("--weight-add", "0"),
    ("--weight-gemm", "1"),
    ("--weight-reduce-max", "0"),
    ("--init-rank-min", "2"),
    ("--init-rank-max", "2"),
    ("--init-dim-min", "2"),
    ("--init-dim-max", "2"),
    ("--expand-ratio", "1"),
];

struct GeneratorFixture {
    _directory: tempfile::TempDir,
    platform_path: PathBuf,
    timetable_path: PathBuf,
}

impl GeneratorFixture {
    fn new(platform_yaml: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let platform_path = directory.path().join("platform.yaml");
        let timetable_path = directory.path().join("timetable.yaml");
        fs::write(&platform_path, platform_yaml).unwrap();
        Self {
            _directory: directory,
            platform_path,
            timetable_path,
        }
    }

    fn generate(&self, args: &[(&str, &str)]) -> TimetableFile {
        let output = self.run_generator(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        TimetableFile::from_file(&self.timetable_path).unwrap()
    }

    fn run_generator(&self, args: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
            .arg("--platform")
            .arg(&self.platform_path)
            .arg("--out")
            .arg(&self.timetable_path)
            .args(args.iter().flat_map(|&(flag, value)| [flag, value]))
            .output()
            .unwrap()
    }

    fn run_to_completion(&self, file: TimetableFile) -> Rc<Platform> {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let platform = Rc::new(Platform::from_file(&engine, &clock, &self.platform_path).unwrap());
        let timetable =
            Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
        timetable.preflight().unwrap();
        let dispatcher: Rc<dyn Dispatch> = timetable.clone();
        platform.attach_dispatcher(&dispatcher);
        engine.run().unwrap();
        timetable.check_tasks_complete().unwrap();
        platform
    }
}

fn assert_generation_failed(output: &Output, message: &str) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(message), "{stderr}");
}

fn compute_program(file: &TimetableFile) -> (&str, &ProgramDefinition) {
    let mut computes = file.nodes.iter().filter_map(|node| match node {
        NodeSection::Compute { pe, program, .. } => Some((pe.as_deref().unwrap(), program)),
        _ => None,
    });
    let (pe, program) = computes.next().unwrap();
    assert!(computes.next().is_none());
    let definition = match program {
        ProgramSection::Named(name) => &file.programs[name],
        ProgramSection::Inline(definition) => definition,
    };
    (pe, definition)
}

fn assert_matrix_mode(file: &TimetableFile, pe: &str, instruction: &str, accumulator: DataType) {
    let (assigned_pe, program) = compute_program(file);
    assert_eq!(assigned_pe, pe);
    let ExprSpec::MatrixMac {
        execution: Some(mode),
        ..
    } = &program.assignments[0].expr
    else {
        panic!("expected a declared matrix execution mode");
    };
    assert_eq!(mode.instruction, instruction);
    assert_eq!(mode.accumulator, accumulator);
}

fn tensor_addresses(file: &TimetableFile) -> Vec<u64> {
    let mut addresses = file
        .nodes
        .iter()
        .filter_map(|node| match node {
            NodeSection::Tensor { config, .. } => Some(config.addr),
            _ => None,
        })
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses
}
