// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs::write;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::syntax::{ExprSpec, IndexSpec, ReductionOp};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{NodeSection, TensorConfigSection, TimetableFile};
use tempfile::{TempDir, tempdir};

const BYTES_PER_FP32: usize = size_of::<f32>();
const MATRIX_EXTENT: usize = 4;
const FULL_MATRIX_ACCUMULATOR_BYTES: usize = MATRIX_EXTENT * MATRIX_EXTENT * BYTES_PER_FP32;

struct GeneratorFixture {
    _directory: TempDir,
    platform_path: PathBuf,
    timetable_path: PathBuf,
}

impl GeneratorFixture {
    fn new(platform_yaml: &str) -> Self {
        let directory = tempdir().unwrap();
        let platform_path = directory.path().join("platform.yaml");
        let timetable_path = directory.path().join("timetable.yaml");
        write(&platform_path, platform_yaml).unwrap();
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
            "gen-timetable failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let file = TimetableFile::from_file(&self.timetable_path).unwrap();
        file.validate().unwrap();
        file
    }

    fn run_generator(&self, args: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_gen-timetable"))
            .arg("--platform")
            .arg(&self.platform_path)
            .arg("--out")
            .arg(&self.timetable_path)
            .args(args.iter().flat_map(|(flag, value)| [*flag, *value]))
            .output()
            .unwrap()
    }

    fn run_to_completion(&self, file: TimetableFile) {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let platform = Rc::new(Platform::from_file(&engine, &clock, &self.platform_path).unwrap());
        let timetable =
            Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
        let dispatcher: Rc<dyn Dispatch> = timetable.clone();
        platform.attach_dispatcher(&dispatcher);
        engine.run().unwrap();
        timetable.check_tasks_complete().unwrap();
    }
}

fn assert_generation_failed(output: &Output, message: &str) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(message), "{stderr}");
}

fn assert_add_views_match_output_partitions(file: &TimetableFile) {
    for node in &file.nodes {
        if let NodeSection::Compute {
            program,
            input_views,
            output_views,
            tiling,
            ..
        } = node
        {
            assert_eq!(program.inputs, ["A", "B"]);
            let input_views = input_views.as_ref().unwrap();
            let output_view = output_views.as_ref().unwrap()[0].as_ref().unwrap();
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
        }
    }
}

fn assert_gemm_partitions_share_rhs(file: &TimetableFile) {
    for node in &file.nodes {
        if let NodeSection::Compute {
            input_views,
            output_views,
            ..
        } = node
        {
            let inputs = input_views.as_ref().unwrap();
            let output = output_views.as_ref().unwrap()[0].as_ref().unwrap();
            let lhs = inputs[0].as_ref().unwrap();
            let rhs = inputs[1].as_ref().unwrap();
            assert_eq!(
                (lhs.offsets[0], lhs.shape[0]),
                (output.offsets[0], output.shape[0])
            );
            assert_eq!(rhs.offsets, vec![0, 0]);
        }
    }
}

fn assert_output_partitions(file: &TimetableFile, expected: &[(usize, usize)]) {
    let mut partitions = compute_nodes(file)
        .map(|node| {
            let NodeSection::Compute { output_views, .. } = node else {
                unreachable!()
            };
            let view = output_views.as_ref().unwrap()[0].as_ref().unwrap();
            (view.offsets[0], view.shape[0])
        })
        .collect::<Vec<_>>();
    partitions.sort_unstable();
    assert_eq!(partitions, expected);
}

fn assert_pe_assignments(file: &TimetableFile, expected: &[&str]) {
    let assignments = compute_nodes(file)
        .map(|node| {
            let NodeSection::Compute { pe, .. } = node else {
                unreachable!()
            };
            pe.as_deref().unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(assignments, expected);
}

fn assert_contraction_is_tiled(file: &TimetableFile) {
    let tiling = compute_nodes(file)
        .find_map(|node| {
            let NodeSection::Compute { tiling, .. } = node else {
                unreachable!()
            };
            tiling.as_ref()
        })
        .unwrap();
    assert!(tiling.sizes.contains_key("k"));
}

fn assert_operations_use_supported_pe(file: &TimetableFile, operation: &str, expected_pe: &str) {
    let mut count = 0;
    for node in compute_nodes(file) {
        let NodeSection::Compute { pe, program, .. } = node else {
            unreachable!()
        };
        count += 1;
        assert_eq!(pe.as_deref(), Some(expected_pe));
        if operation == "gemm" {
            assert_eq!(program.inputs.len(), 2, "bias needs add support");
            assert!(matches!(
                program.assignments[0].expr,
                ExprSpec::MatrixMac {
                    accumulator: DataType::Fp32,
                    ..
                }
            ));
        }
    }
    assert_eq!(count, 4);
}

fn assert_generated_operation(file: &TimetableFile, expected: &str) {
    let mut count = 0;
    for node in compute_nodes(file) {
        let NodeSection::Compute { program, .. } = node else {
            unreachable!()
        };
        count += 1;
        assert!(match (&program.assignments[0].expr, expected) {
            (ExprSpec::Call { op, .. }, "add") => op == "add",
            (ExprSpec::MatrixMac { .. }, "gemm") => true,
            _ => false,
        });
    }
    assert!(count > 0);
}

fn assert_max_reduction_outputs(file: &TimetableFile) {
    let mut saw_reduction = false;
    let mut saw_indices = false;
    for node in compute_nodes(file) {
        let NodeSection::Compute { program, .. } = node else {
            unreachable!()
        };
        match &program.assignments[0].expr {
            ExprSpec::Call { .. } | ExprSpec::MatrixMac { .. } => {}
            ExprSpec::Reduce {
                op: ReductionOp::Min | ReductionOp::Max,
                ..
            } => {
                saw_reduction = true;
                saw_indices |= assert_reduction_tensor_shapes(file, node);
            }
            other => panic!("unexpected generated expression {other:?}"),
        }
    }
    assert!(
        saw_reduction,
        "generated timetable did not contain max reduction"
    );
    assert!(
        saw_indices,
        "generated timetable did not contain max-reduction indices output"
    );
}

fn compute_nodes(file: &TimetableFile) -> impl Iterator<Item = &NodeSection> {
    file.nodes
        .iter()
        .filter(|node| matches!(node, NodeSection::Compute { .. }))
}

fn assert_reduction_tensor_shapes(file: &TimetableFile, node: &NodeSection) -> bool {
    let NodeSection::Compute {
        id,
        program,
        output_views,
        ..
    } = node
    else {
        unreachable!()
    };
    let ExprSpec::Reduce { over, value, .. } = &program.assignments[0].expr else {
        unreachable!()
    };
    let output_views = output_views.as_ref().unwrap();
    assert_eq!(over, &["k"]);
    assert!(
        matches!(&**value, ExprSpec::Read { indices, .. } if matches!(indices.last(), Some(IndexSpec::Axis(axis)) if axis == "k"))
    );
    let input_edge = file
        .edges
        .iter()
        .find(|edge| edge.to == format!("{id}.0"))
        .unwrap_or_else(|| panic!("missing input edge for {id}"));
    let input = tensor_config(file, &input_edge.from);
    assert!((1..=2).contains(&output_views.len()));
    assert_eq!(program.outputs.len(), output_views.len());

    let values_edge = file
        .edges
        .iter()
        .find(|edge| edge.from == format!("{id}.0"))
        .unwrap_or_else(|| panic!("missing values edge for {id}"));
    let values = tensor_config(file, &values_edge.to);
    assert_eq!(values.shape, input.shape[..input.shape.len() - 1]);
    if output_views.len() == 2 {
        let indices_edge = file
            .edges
            .iter()
            .find(|edge| edge.from == format!("{id}.1"))
            .unwrap_or_else(|| panic!("missing indices edge for {id}"));
        let indices = tensor_config(file, &indices_edge.to);
        assert_eq!(indices.dtype, DataType::Int64);
        assert_eq!(indices.shape, values.shape);
    }
    output_views.len() == 2
}

fn three_pe_platform() -> String {
    PLATFORM_YAML
        .replace(
            "memories:\n",
            "  - name: pe1
    memory_map: default
    config:
      compute:
        execution_units:
          vector:
            capabilities: [{ class: add, ops_per_tick: 256 }]
  - name: pe2
    memory_map: default
    config:
      compute:
        execution_units:
          vector:
            capabilities: [{ class: add, ops_per_tick: 256 }]

fabrics:
  - name: fabric0
    kind: functional
    columns: 2
    rows: 2
    config: { routing: column-first }

memories:
",
        )
        .replace(
            "connections:
  - connect: [pe.pe0, mem.hbm0]",
            "connections:
  - connect: [pe.pe0, 'fabric.fabric0@(0,0)']
  - connect: [pe.pe1, 'fabric.fabric0@(0,1)']
  - connect: [pe.pe2, 'fabric.fabric0@(1,0)']
  - connect: [mem.hbm0, 'fabric.fabric0@(1,1)']",
        )
}

fn platform_with_sram(bytes: usize) -> String {
    PLATFORM_YAML.replace(
        "    config:\n      compute:",
        &format!("    config:\n      sram_bytes: {bytes}\n      compute:"),
    )
}

fn matrix_capacity_platform(first_capacity: usize, second_capacity: usize) -> String {
    format!(
        r"
memory_maps:
  - name: default
    devices: [hbm0]
operation_classes:
  matrix: {{ operations: [matrix_mac], arity: 2, flops_per_op: 2 }}
processing_elements:
  - name: pe0
    memory_map: default
    config:
      compute:
        execution_units:
          matrix:
            matrix_accumulator_capacity_bytes: {first_capacity}
            capabilities:
              - class: matrix
                input_dtypes: [[fp32, fp32]]
                accumulator: fp32
                ops_per_tick: 256
  - name: pe1
    memory_map: default
    config:
      compute:
        execution_units:
          matrix:
            matrix_accumulator_capacity_bytes: {second_capacity}
            capabilities:
              - class: matrix
                input_dtypes: [[fp32, fp32]]
                accumulator: fp32
                ops_per_tick: 256
fabrics:
  - name: fabric0
    kind: functional
    columns: 3
    rows: 1
    config: {{}}
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config: {{ capacity_bytes: 1GiB }}
connections:
  - connect: [pe.pe0, 'fabric.fabric0@(0,0)']
  - connect: [pe.pe1, 'fabric.fabric0@(1,0)']
  - connect: [mem.hbm0, 'fabric.fabric0@(2,0)']
"
    )
}

fn generate_square_gemm(fixture: &GeneratorFixture, compute_nodes: usize) -> TimetableFile {
    let extent = MATRIX_EXTENT.to_string();
    let nodes = compute_nodes.to_string();
    let partition_size = MATRIX_EXTENT.div_ceil(compute_nodes).to_string();
    fixture.generate(&[
        ("--depth", "1"),
        ("--max-compute-nodes", &nodes),
        ("--partition-size", &partition_size),
        ("--weight-add", "0"),
        ("--weight-gemm", "1"),
        ("--weight-reduce-max", "0"),
        ("--expand-ratio", "1"),
        ("--init-rank-min", "2"),
        ("--init-rank-max", "2"),
        ("--init-dim-min", &extent),
        ("--init-dim-max", &extent),
    ])
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
        execution_units:
          vector:
            capabilities:
              - { class: add, ops_per_tick: 256 }
              - { class: mul, ops_per_tick: 256 }
              - { class: compare, ops_per_tick: 256 }
          matrix:
            capabilities: [{ class: matrix, accumulator: fp32, ops_per_tick: 1024 }]

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
      capacity_bytes: 0x1000_0000
connections:
  - connect: [pe.pe0, mem.hbm0]
";

const HETEROGENEOUS_PLATFORM_YAML: &str = "
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
        execution_units:
          alu:
            capabilities: [{ class: add, ops_per_tick: 16 }]
  - name: pe_matrix
    memory_map: default
    config:
      compute:
        execution_units:
          matrix:
            capabilities: [{ class: matrix, accumulator: fp32, ops_per_tick: 256 }]
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

const FP16_ADD_PLATFORM_YAML: &str = "
memory_maps: [{ name: default, devices: [hbm0] }]
operation_classes:
  add: { operations: [add], arity: 2 }
processing_elements:
  - name: pe0
    memory_map: default
    config:
      compute:
        execution_units:
          alu:
            capabilities:
              - { class: add, input_dtypes: [[fp16, fp16]], ops_per_tick: 16 }
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config: { capacity_bytes: 1GiB }
connections: [{ connect: [pe.pe0, mem.hbm0] }]
";

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
fn generator_partitions_add_across_pes_with_disjoint_views() {
    let fixture = GeneratorFixture::new(&three_pe_platform());
    let file = fixture.generate(&[
        ("--depth", "1"),
        ("--max-compute-nodes", "4"),
        ("--partition-size", "2"),
        ("--weight-add", "1"),
        ("--weight-gemm", "0"),
        ("--weight-reduce-max", "0"),
        ("--init-rank-min", "2"),
        ("--init-rank-max", "2"),
        ("--init-dim-min", "8"),
        ("--init-dim-max", "8"),
    ]);

    assert_add_views_match_output_partitions(&file);
    assert_output_partitions(&file, &[(0, 2), (2, 2), (4, 2), (6, 2)]);
    assert_pe_assignments(&file, &["pe0", "pe1", "pe2", "pe0"]);
    fixture.run_to_completion(file);
}

#[test]
fn generator_partitions_gemm_without_slicing_shared_rhs() {
    let fixture = GeneratorFixture::new(PLATFORM_YAML);
    let file = fixture.generate(&[
        ("--depth", "1"),
        ("--max-compute-nodes", "4"),
        ("--partition-size", "2"),
        ("--weight-add", "0"),
        ("--weight-gemm", "1"),
        ("--weight-reduce-max", "0"),
        ("--expand-ratio", "0.5"),
        ("--init-rank-min", "2"),
        ("--init-rank-max", "2"),
        ("--init-dim-min", "8"),
        ("--init-dim-max", "8"),
    ]);

    assert_gemm_partitions_share_rhs(&file);
    assert_output_partitions(&file, &[(0, 2), (2, 2), (4, 2), (6, 2)]);
    fixture.run_to_completion(file);
}

#[test]
fn generator_tiles_gemm_to_fit_pe_sram() {
    let fixture = GeneratorFixture::new(&platform_with_sram(256));
    let file = fixture.generate(&[
        ("--depth", "1"),
        ("--max-compute-nodes", "1"),
        ("--weight-add", "0"),
        ("--weight-gemm", "1"),
        ("--weight-reduce-max", "0"),
        ("--expand-ratio", "1"),
        ("--partition-size", "8"),
        ("--init-rank-min", "2"),
        ("--init-rank-max", "2"),
        ("--init-dim-min", "8"),
        ("--init-dim-max", "8"),
    ]);

    assert_contraction_is_tiled(&file);
    fixture.run_to_completion(file);
}

#[test]
fn generator_tiles_gemm_to_fit_matrix_accumulator_capacity() {
    // SRAM can hold the complete matrix, but the accumulator holds one FP32
    // value.
    let fixture = GeneratorFixture::new(&matrix_capacity_platform(BYTES_PER_FP32, BYTES_PER_FP32));
    let file = generate_square_gemm(&fixture, 1);

    fixture.run_to_completion(file);
}

#[test]
fn generator_plans_each_pes_tiles_for_its_matrix_accumulator_capacity() {
    // Equal SRAM and partition sizes must not share a tiling that exceeds pe1's
    // accumulator.
    let fixture = GeneratorFixture::new(&matrix_capacity_platform(
        FULL_MATRIX_ACCUMULATOR_BYTES,
        BYTES_PER_FP32,
    ));
    let file = generate_square_gemm(&fixture, 2);

    assert_pe_assignments(&file, &["pe0", "pe1"]);
    fixture.run_to_completion(file);
}

#[test]
fn generator_rejects_a_tile_that_cannot_fit_even_at_unit_size() {
    let fixture = GeneratorFixture::new(&platform_with_sram(8));
    let output = fixture.run_generator(&[
        ("--depth", "1"),
        ("--max-compute-nodes", "1"),
        ("--weight-add", "1"),
        ("--weight-gemm", "0"),
        ("--weight-reduce-max", "0"),
        ("--init-rank-min", "2"),
        ("--init-rank-max", "2"),
        ("--init-dim-min", "8"),
        ("--init-dim-max", "8"),
    ]);

    assert_generation_failed(&output, "minimum tile requires");
    assert!(!fixture.timetable_path.exists());
}

#[test]
fn generator_places_only_supported_operations_on_heterogeneous_pes() {
    let fixture = GeneratorFixture::new(HETEROGENEOUS_PLATFORM_YAML);
    for (name, add_weight, gemm_weight, expected_pe) in
        [("add", "1", "0", "pe_add"), ("gemm", "0", "1", "pe_matrix")]
    {
        let file = fixture.generate(&[
            ("--depth", "1"),
            ("--max-compute-nodes", "4"),
            ("--partition-size", "2"),
            ("--weight-add", add_weight),
            ("--weight-gemm", gemm_weight),
            ("--weight-reduce-max", "0"),
            ("--expand-ratio", "1"),
            ("--init-rank-min", "2"),
            ("--init-rank-max", "2"),
            ("--init-dim-min", "8"),
            ("--init-dim-max", "8"),
        ]);

        assert_operations_use_supported_pe(&file, name, expected_pe);
        fixture.run_to_completion(file);
    }
}

#[test]
fn generator_default_partition_count_uses_only_eligible_pes() {
    let fixture = GeneratorFixture::new(HETEROGENEOUS_PLATFORM_YAML);
    let file = fixture.generate(&[
        ("--depth", "1"),
        ("--max-compute-nodes", "4"),
        ("--weight-add", "0"),
        ("--weight-gemm", "1"),
        ("--weight-reduce-max", "0"),
        ("--init-rank-min", "2"),
        ("--init-rank-max", "2"),
        ("--init-dim-min", "8"),
        ("--init-dim-max", "8"),
    ]);
    assert_eq!(
        compute_nodes(&file).count(),
        1,
        "default partition count uses eligible PEs"
    );
}

#[test]
fn generator_rejects_templates_with_no_supported_operation() {
    let fixture = GeneratorFixture::new(FP16_ADD_PLATFORM_YAML);
    let output = fixture.run_generator(&[
        ("--weight-add", "0"),
        ("--weight-gemm", "1"),
        ("--weight-reduce-max", "0"),
        ("--init-dtype", "fp16"),
    ]);
    assert_generation_failed(&output, "no supported compute templates");
}

#[test]
fn generator_rejects_templates_with_no_supported_dtype() {
    let fixture = GeneratorFixture::new(FP16_ADD_PLATFORM_YAML);
    let output = fixture.run_generator(&[
        ("--weight-add", "1"),
        ("--weight-gemm", "0"),
        ("--weight-reduce-max", "0"),
        ("--init-dtype", "fp32"),
    ]);
    assert_generation_failed(&output, "no supported compute templates");
}

#[test]
fn generator_uses_supported_template_when_another_template_is_unsupported() {
    let fixture = GeneratorFixture::new(FP16_ADD_PLATFORM_YAML);
    let file = fixture.generate(&[
        ("--weight-add", "1"),
        ("--weight-gemm", "1"),
        ("--weight-reduce-max", "0"),
        ("--init-dtype", "fp16"),
    ]);
    assert_generated_operation(&file, "add");
}

#[test]
fn generator_emits_multi_output_reduce_max() {
    let fixture = GeneratorFixture::new(&platform_with_sram(256));
    let file = fixture.generate(&[
        ("--depth", "6"),
        ("--max-compute-nodes", "8"),
        ("--partition-size", "2"),
        ("--expand-ratio", "1.0"),
        ("--weight-add", "0.0"),
        ("--weight-gemm", "0.0"),
        ("--weight-reduce-max", "1.0"),
        ("--seed", "1592612472"),
        ("--init-rank-min", "4"),
        ("--init-rank-max", "4"),
        ("--init-dim-min", "4"),
        ("--init-dim-max", "4"),
    ]);

    assert_max_reduction_outputs(&file);
    fixture.run_to_completion(file);
}

#[test]
fn generator_emits_valid_reduction_program() {
    let fixture = GeneratorFixture::new(PLATFORM_YAML);
    fixture.generate(&[
        ("--max-compute-nodes", "1"),
        ("--expand-ratio", "2.0"),
        ("--weight-add", "0"),
        ("--weight-gemm", "0"),
        ("--weight-reduce-max", "1"),
        ("--init-rank-min", "4"),
        ("--init-rank-max", "4"),
        ("--init-dim-min", "2"),
        ("--init-dim-max", "2"),
    ]);
}

#[test]
fn generator_emits_valid_add_and_matrix_programs() {
    let fixture = GeneratorFixture::new(PLATFORM_YAML);
    for (name, add_weight, gemm_weight) in [("add", "1", "0"), ("gemm", "0", "1")] {
        let file = fixture.generate(&[
            ("--max-compute-nodes", "3"),
            ("--weight-add", add_weight),
            ("--weight-gemm", gemm_weight),
            ("--weight-reduce-max", "0"),
            ("--init-rank-min", "2"),
            ("--init-rank-max", "2"),
        ]);
        assert_generated_operation(&file, name);
    }
}

const SMALL_ADD_ARGS: &[(&str, &str)] = &[
    ("--depth", "1"),
    ("--max-compute-nodes", "1"),
    ("--weight-add", "1"),
    ("--weight-gemm", "0"),
    ("--weight-reduce-max", "0"),
    ("--init-rank-min", "1"),
    ("--init-rank-max", "1"),
    ("--init-dim-min", "4"),
    ("--init-dim-max", "4"),
    ("--tensor-align-bytes", "32"),
];

#[test]
fn generator_tries_later_pe_when_first_pe_cannot_fit() {
    let yaml = three_pe_platform().replacen(
        "    config:\n      compute:",
        "    config:\n      sram_bytes: 1\n      compute:",
        1,
    );
    let fixture = GeneratorFixture::new(&yaml);
    let file = fixture.generate(SMALL_ADD_ARGS);
    assert_pe_assignments(&file, &["pe1"]);
    fixture.run_to_completion(file);
}

#[test]
fn generator_allocates_in_selected_pe_map() {
    let yaml = PLATFORM_YAML.replace("memories:\n", "memories:\n  - name: unused\n    kind: hbm\n    base_address: 0\n    config: { capacity_bytes: 4096 }\n").replace("    base_address: 0\n    config:\n", "    base_address: 4096\n    config:\n");
    let yaml = yaml.replace("connections:\n  - connect: [pe.pe0, mem.hbm0]", "fabrics:\n  - name: fabric0\n    kind: functional\n    columns: 3\n    rows: 1\n    config: {}\nconnections:\n  - connect: [pe.pe0, 'fabric.fabric0@(0,0)']\n  - connect: [mem.hbm0, 'fabric.fabric0@(1,0)']\n  - connect: [mem.unused, 'fabric.fabric0@(2,0)']");
    let fixture = GeneratorFixture::new(&yaml);
    let file = fixture.generate(SMALL_ADD_ARGS);
    let mut addresses = file
        .nodes
        .iter()
        .filter_map(|node| match node {
            NodeSection::Tensor { config, .. } => Some(config.addr),
            NodeSection::Compute { .. } => None,
        })
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    assert_eq!(addresses, [4096, 4128, 4160]);
    fixture.run_to_completion(file);
}

#[test]
fn generator_checks_physical_capacity_and_exact_fit() {
    for (capacity, succeeds) in [(64, false), (80, true)] {
        let yaml = PLATFORM_YAML
            .replace(
                "capacity_bytes: 0x1000_0000",
                &format!("capacity_bytes: {capacity}"),
            )
            .replace(
                "    config:\n      compute:",
                "    config:\n      lsu_access_bytes: 1\n      compute:",
            );
        let fixture = GeneratorFixture::new(&yaml);
        if succeeds {
            let file = fixture.generate(SMALL_ADD_ARGS);
            let mut addresses = file
                .nodes
                .iter()
                .filter_map(|node| match node {
                    NodeSection::Tensor { config, .. } => Some(config.addr),
                    NodeSection::Compute { .. } => None,
                })
                .collect::<Vec<_>>();
            addresses.sort_unstable();
            assert_eq!(addresses, [0, 32, 64]);
            fixture.run_to_completion(file);
        } else {
            assert_generation_failed(&fixture.run_generator(SMALL_ADD_ARGS), "placement");
            assert!(!fixture.timetable_path.exists());
        }
    }
}

#[test]
fn generator_preserves_shared_rhs_across_selected_memory_maps() {
    let yaml = matrix_capacity_platform(64, 64)
        .replace("  - name: default\n    devices: [hbm0]", "  - { name: first, devices: [a, c] }\n  - { name: second, devices: [b, c] }\n  - { name: unused, devices: [d] }")
        .replacen("memory_map: default", "memory_map: first", 1)
        .replacen("memory_map: default", "memory_map: second", 1)
        .replace("fabrics:\n", "  - name: unused\n    memory_map: unused\n    config:\n      compute:\n        execution_units:\n          vector:\n            capabilities: [{ class: add, ops_per_tick: 1 }]\nfabrics:\n")
        .replace("operation_classes:\n", "operation_classes:\n  add: { operations: [add], arity: 2 }\n")
        .replace("columns: 3", "columns: 7")
        .replace("  - name: hbm0\n    kind: hbm\n    base_address: 0\n    config: { capacity_bytes: 1GiB }", "  - { name: a, kind: hbm, base_address: 0, config: { capacity_bytes: 4096 } }\n  - { name: b, kind: hbm, base_address: 4096, config: { capacity_bytes: 4096 } }\n  - { name: c, kind: hbm, base_address: 8192, config: { capacity_bytes: 4096 } }\n  - { name: d, kind: hbm, base_address: 12288, config: { capacity_bytes: 4096 } }")
        .replace("  - connect: [mem.hbm0, 'fabric.fabric0@(2,0)']", "  - connect: [pe.unused, 'fabric.fabric0@(2,0)']\n  - connect: [mem.a, 'fabric.fabric0@(3,0)']\n  - connect: [mem.b, 'fabric.fabric0@(4,0)']\n  - connect: [mem.c, 'fabric.fabric0@(5,0)']\n  - connect: [mem.d, 'fabric.fabric0@(6,0)']");
    let fixture = GeneratorFixture::new(&yaml);
    let file = generate_square_gemm(&fixture, 2);
    assert_pe_assignments(&file, &["pe0", "pe1"]);
    assert_gemm_partitions_share_rhs(&file);
    let rhs_ids = compute_nodes(&file)
        .map(|node| {
            let NodeSection::Compute { id, .. } = node else {
                unreachable!()
            };
            &file
                .edges
                .iter()
                .find(|edge| edge.to == format!("{id}.1"))
                .unwrap()
                .from
        })
        .collect::<Vec<_>>();
    assert_eq!(rhs_ids[0], rhs_ids[1]);
    let rhs = tensor_config(&file, rhs_ids[0]);
    assert_eq!(rhs.shape, [4, 4]);
    assert!(
        (8192..=12224).contains(&rhs.addr),
        "shared 64-byte RHS must fit memory c: {rhs:?}"
    );
    assert_eq!(
        file.nodes
            .iter()
            .filter(|node| matches!(node, NodeSection::Tensor { id, .. } if id == rhs_ids[0]))
            .count(),
        1
    );
    fixture.run_to_completion(file);
}

#[test]
fn generator_reports_all_resource_infeasible_pe_candidates() {
    let yaml = three_pe_platform().replace(
        "    config:\n      compute:",
        "    config:\n      sram_bytes: 1\n      compute:",
    );
    let fixture = GeneratorFixture::new(&yaml);
    let output = fixture.run_generator(SMALL_ADD_ARGS);
    assert_generation_failed(
        &output,
        "no feasible placement found for this generated mapping",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    for pe in ["pe0", "pe1", "pe2"] {
        assert!(stderr.contains(&format!("{pe}:")), "{stderr}");
    }
    assert!(stderr.contains("SRAM bytes, available 1"), "{stderr}");
    assert!(!fixture.timetable_path.exists());
}
