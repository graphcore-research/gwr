// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

fn platform(workspace: usize) -> String {
    format!(
        r"
operation_classes:
 matrix: {{operations: [matrix_mac], arity: 2, flops_per_op: 2}}
 cast: {{operations: [convert], arity: 1, flops_per_op: 0}}
memory_maps: [{{name: all, devices: [hbm]}}]
processing_elements:
 - name: pe
   memory_map: all
   config:
    num_active_requests: 32
    lsu_access_bytes: 64
    overhead_size_bytes: 0
    sram_bytes: {workspace}
    block_execution:
     max_compute_tiles: 1
     max_load_chunks: 1
     max_store_tiles: 1
     transfer_port: lsu
     accumulator_storage: shared_sram
    compute:
     engines:
      matrix:
       matrix_accumulator_capacity_bytes: 256
       capabilities: [{{class: matrix, input_dtypes: [[fp8,fp8]], ops_per_tick: 8, execution: {{instruction: test, accumulator: fp32, sparsity: dense}}}}]
      vector:
       capabilities: [{{class: cast, input_dtypes: [[fp32]], ops_per_tick: 8}}]
memories:
 - {{name: hbm, kind: hbm, base_address: 0, config: {{capacity_bytes: 65536, bw_bytes_per_tick: 16, delay_ticks: 5}}}}
connections:
 - {{connect: [pe.pe, mem.hbm]}}
"
    )
}

fn workload(mode: &str, inputs: usize, outputs: usize, distance: usize) -> String {
    format!(
        r"
nodes:
 - {{id: A, kind: tensor, config: {{addr: 0, dtype: fp8, shape: [2,8,16]}}}}
 - {{id: B, kind: tensor, config: {{addr: 4096, dtype: fp8, shape: [2,16,8]}}}}
 - {{id: C, kind: tensor, config: {{addr: 8192, dtype: fp16, shape: [2,8,8]}}}}
 - id: gemm
   kind: compute
   pe: pe
   tiling: {{sizes: {{b: 1, m: 8, n: 8, k: 4}}}}
   pipeline: {{mode: {mode}, input_buffers: {inputs}, output_buffers: {outputs}, prefetch_distance: {distance}}}
   program:
    inputs: [A,B]
    outputs: [C]
    temporaries: {{P: {{dtype: fp32}}}}
    assignments:
     - output: P
       axes: [b,m,n]
       expr: {{matrix_mac: {{over: [k], execution: {{instruction: test, accumulator: fp32, sparsity: dense}}, args: [{{read: {{tensor: A, indices: [b,m,k]}}}}, {{read: {{tensor: B, indices: [b,k,n]}}}}]}}}}
     - output: C
       axes: [b,m,n]
       expr: {{convert: [{{read: {{tensor: P, indices: [b,m,n]}}}}]}}
edges:
 - {{from: A, to: gemm.0, kind: data}}
 - {{from: B, to: gemm.1, kind: data}}
 - {{from: gemm.0, to: C, kind: data}}
"
    )
}

fn run(
    mode: &str,
    inputs: usize,
    outputs: usize,
    distance: usize,
    workspace: usize,
) -> (u64, (usize, usize), usize) {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform(workspace)).unwrap());
    let file = TimetableFile::from_string(&workload(mode, inputs, outputs, distance)).unwrap();
    let table =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    table.preflight().unwrap();
    let dispatch: Rc<dyn Dispatch> = table.clone();
    platform.attach_dispatcher(&dispatch);
    engine.run().unwrap();
    table.check_tasks_complete().unwrap();
    let pe = platform.pe("pe").unwrap();
    assert_eq!(pe.machine_ops().by_operation["matrix_mac"], 2048);
    assert_eq!(pe.machine_ops().by_operation["convert"], 128);
    (
        clock.tick_now().tick(),
        pe.memory_traffic_bytes(),
        pe.peak_sram_bytes(),
    )
}

#[test]
fn overlap_preserves_work_and_reduces_balanced_gemm_time() {
    let serial = run("serial", 2, 1, 1, 4096);
    let overlap = run("overlap", 2, 1, 1, 4096);
    assert_eq!(serial.1, (512, 256));
    assert_eq!(overlap.1, serial.1);
    assert_eq!(overlap.2, 512);
    assert!(
        overlap.0 < serial.0,
        "serial={serial:?} overlap={overlap:?}"
    );
}

#[test]
fn arbitrary_buffer_counts_are_all_charged_at_exact_fit() {
    // Five complete 64-byte input slots + four 128-byte output slots + 256B
    // state.
    let result = run("overlap", 5, 4, 4, 1088);
    assert_eq!(result.2, 1088);
    assert_eq!(result.1, (512, 256));
}

#[test]
fn full_window_rejects_one_byte_short_before_effects() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(Platform::from_string(&engine, &clock, &platform(1087)).unwrap());
    let file = TimetableFile::from_string(&workload("overlap", 5, 4, 4)).unwrap();
    let table = Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap();
    let error = table.preflight().unwrap_err().to_string();
    assert!(error.contains("1088") && error.contains("1087"), "{error}");
    assert_eq!(clock.tick_now().tick(), 0);
    assert_eq!(p.pe("pe").unwrap().memory_traffic_bytes(), (0, 0));
}

fn assert_preflight_rejects(platform_yaml: &str, timetable_yaml: &str, expected: &str) {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(Platform::from_string(&engine, &clock, platform_yaml).unwrap());
    let file = TimetableFile::from_string(timetable_yaml).unwrap();
    let table = Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap();
    let error = table.preflight().unwrap_err().to_string();
    assert!(error.contains(expected), "expected {expected:?}: {error}");
    let pe = p.pe("pe").unwrap();
    assert_eq!(clock.tick_now().tick(), 0);
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert_eq!(pe.machine_ops().by_operation.values().sum::<usize>(), 0);
}

#[test]
fn selected_pipeline_requires_platform_support_without_fallback() {
    let mut yaml: yaml_serde::Value = yaml_serde::from_str(&platform(4096)).unwrap();
    yaml["processing_elements"][0]["config"]
        .as_mapping_mut()
        .unwrap()
        .remove(yaml_serde::Value::String("block_execution".into()));
    assert_preflight_rejects(
        &yaml_serde::to_string(&yaml).unwrap(),
        &workload("overlap", 2, 1, 1),
        "block_execution",
    );
}

#[test]
fn selected_pipeline_requires_qualified_accumulator_before_effects() {
    let yaml = platform(4096).replace(
        "instruction: test, accumulator: fp32",
        "instruction: other, accumulator: fp32",
    );
    assert_preflight_rejects(&yaml, &workload("overlap", 2, 1, 1), "matrix");
}

#[test]
fn runtime_also_rejects_impossible_window_when_preflight_is_omitted() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(Platform::from_string(&engine, &clock, &platform(1087)).unwrap());
    let file = TimetableFile::from_string(&workload("overlap", 5, 4, 4)).unwrap();
    let table = Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap());
    let dispatch: Rc<dyn Dispatch> = table.clone();
    p.attach_dispatcher(&dispatch);
    let error = engine.run().unwrap_err().to_string();
    assert!(error.contains("1088") && error.contains("1087"), "{error}");
    assert_eq!(clock.tick_now().tick(), 0);
    assert_eq!(p.pe("pe").unwrap().memory_traffic_bytes(), (0, 0));
    assert!(table.check_tasks_complete().is_err());
}

#[test]
fn downstream_consumer_waits_for_pipeline_store_completion() {
    use std::collections::BTreeMap;

    use gwr_timetable::scheduled::ScheduledTimetable;
    let source = workload("overlap", 5, 4, 4)
        .replace("edges:", " - {id: D, kind: tensor, config: {addr: 12288, dtype: fp16, shape: [2,8,8]}}\n - {id: consumer, kind: transfer, pe: pe, buffer_bytes: 64}\nedges:")
        + " - {from: C, to: consumer, kind: data}\n - {from: consumer, to: D, kind: data}\n";
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(Platform::from_string(&engine, &clock, &platform(4096)).unwrap());
    let file = TimetableFile::from_string(&source).unwrap();
    let table = Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap());
    table.preflight().unwrap();
    let scheduled =
        Rc::new(ScheduledTimetable::new(table.clone(), clock, BTreeMap::new()).unwrap());
    let dispatch: Rc<dyn Dispatch> = scheduled.clone();
    p.attach_dispatcher(&dispatch);
    engine.run().unwrap();
    table.check_tasks_complete().unwrap();
    let times = scheduled.timings();
    assert!(times["consumer"].started_tick.unwrap() >= times["gemm"].completed_tick.unwrap());
    assert_eq!(p.pe("pe").unwrap().memory_traffic_bytes(), (768, 512));
}

#[test]
fn serial_and_overlap_preserve_transposed_tail_work_and_traffic() {
    fn execute(mode: &str) -> (usize, usize, (usize, usize), usize) {
        let source = workload(mode, 3, 2, 2)
            .replace("[2,8,16]", "[2,5,11]")
            .replace("[2,16,8]", "[2,7,11]")
            .replace("[2,8,8]", "[2,5,7]")
            .replace("m: 8, n: 8, k: 4", "m: 3, n: 4, k: 4")
            .replace("tensor: B, indices: [b,k,n]", "tensor: B, indices: [b,n,k]");
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let p = Rc::new(Platform::from_string(&engine, &clock, &platform(4096)).unwrap());
        let file = TimetableFile::from_string(&source).unwrap();
        let table = Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap());
        table.preflight().unwrap();
        let dispatch: Rc<dyn Dispatch> = table.clone();
        p.attach_dispatcher(&dispatch);
        engine.run().unwrap();
        table.check_tasks_complete().unwrap();
        let pe = p.pe("pe").unwrap();
        (
            pe.machine_ops().by_operation["matrix_mac"],
            pe.machine_ops().by_operation["convert"],
            pe.memory_traffic_bytes(),
            pe.peak_sram_bytes(),
        )
    }
    let serial = execute("serial");
    let overlap = execute("overlap");
    assert_eq!(serial, overlap);
    assert_eq!((overlap.0, overlap.1, overlap.2), (770, 70, (528, 140)));
}

#[test]
fn two_pipeline_nodes_share_stage_limits_and_sram() {
    let mut doc: yaml_serde::Value = yaml_serde::from_str(&workload("overlap", 2, 1, 1)).unwrap();
    let mut output = doc["nodes"][2].clone();
    output["id"] = yaml_serde::Value::String("D".into());
    output["config"]["addr"] = yaml_serde::to_value(12288usize).unwrap();
    let mut compute = doc["nodes"][3].clone();
    compute["id"] = yaml_serde::Value::String("second".into());
    compute["program"]["outputs"][0] = yaml_serde::Value::String("D".into());
    compute["program"]["assignments"][1]["output"] = yaml_serde::Value::String("D".into());
    doc["nodes"]
        .as_sequence_mut()
        .unwrap()
        .extend([output, compute]);
    let more: yaml_serde::Value = yaml_serde::from_str("[{from: A, to: second.0, kind: data}, {from: B, to: second.1, kind: data}, {from: second.0, to: D, kind: data}]").unwrap();
    doc["edges"]
        .as_sequence_mut()
        .unwrap()
        .extend(more.as_sequence().unwrap().iter().cloned());
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(Platform::from_string(&engine, &clock, &platform(1024)).unwrap());
    let file = TimetableFile::from_string(&yaml_serde::to_string(&doc).unwrap()).unwrap();
    let table = Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap());
    table.preflight().unwrap();
    let dispatch: Rc<dyn Dispatch> = table.clone();
    p.attach_dispatcher(&dispatch);
    engine.run().unwrap();
    table.check_tasks_complete().unwrap();
    let pe = p.pe("pe").unwrap();
    assert_eq!(pe.machine_ops().by_operation["matrix_mac"], 4096);
    assert_eq!(pe.memory_traffic_bytes(), (1024, 512));
    assert_eq!(pe.peak_sram_bytes(), 1024);
    let snapshot = pe.block_execution_snapshot().unwrap();
    assert_eq!(snapshot.peak_load_chunks, 1);
    assert_eq!(snapshot.peak_compute_chunks, 1);
    assert_eq!(snapshot.peak_store_tiles, 1);
    assert_eq!(snapshot.reserved_window_bytes, 0);
    assert_eq!(snapshot.occupied_input_slots, 0);
    assert_eq!(snapshot.occupied_output_slots, 0);
    assert_eq!(snapshot.completed_load_chunks, 16);
    assert_eq!(snapshot.completed_compute_chunks, 16);
    assert_eq!(snapshot.completed_store_tiles, 4);
}

#[test]
fn aligned_tail_packets_must_fit_the_actual_memory_map_before_issue() {
    // B occupies4095..4351: all logical bytes fit, but its final64B
    // packet4288..4352 escapes a map whose exclusive end is4351.
    let source = workload("overlap", 2, 1, 1)
        .replace("addr: 8192", "addr: 1024")
        .replace("addr: 4096", "addr: 4095");
    let yaml = platform(4096).replace("capacity_bytes: 65536", "capacity_bytes: 4351");
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let error = match Platform::from_string(&engine, &clock, &yaml) {
        Ok(_) => panic!("unaligned physical map accepted"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("multiple of the 64-byte fetch size"),
        "{error}"
    );
    assert_eq!(clock.tick_now().tick(), 0);
    let p = Rc::new(
        Platform::from_string(
            &engine,
            &clock,
            &yaml.replace("capacity_bytes: 4351", "capacity_bytes: 4352"),
        )
        .unwrap(),
    );
    let file = TimetableFile::from_string(&source).unwrap();
    let table = Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap());
    table.preflight().unwrap();
    let dispatch: Rc<dyn Dispatch> = table.clone();
    p.attach_dispatcher(&dispatch);
    engine.run().unwrap();
    table.check_tasks_complete().unwrap();
    assert_eq!(p.pe("pe").unwrap().memory_traffic_bytes(), (512, 256));
}

fn observed_pipeline(
    local_rate: Option<u64>,
) -> (u64, gwr_models::processing_element::BlockExecutionSnapshot) {
    let mut yaml = platform(4096);
    if let Some(rate) = local_rate {
        yaml = yaml.replace("accumulator_storage: shared_sram", &format!("accumulator_storage: shared_sram\n     local_memory_service: {{bytes_per_tick: {rate}}}"));
    }
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let p = Rc::new(Platform::from_string(&engine, &clock, &yaml).unwrap());
    let file = TimetableFile::from_string(&workload("overlap", 3, 2, 2)).unwrap();
    let table = Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &p).unwrap());
    table.preflight().unwrap();
    let dispatch: Rc<dyn Dispatch> = table.clone();
    p.attach_dispatcher(&dispatch);
    engine.run().unwrap();
    table.check_tasks_complete().unwrap();
    assert_eq!(p.pe("pe").unwrap().memory_traffic_bytes(), (512, 256));
    (
        clock.tick_now().tick(),
        p.pe("pe").unwrap().block_execution_snapshot().unwrap(),
    )
}

#[test]
fn real_pipeline_records_load_compute_and_store_compute_intersections() {
    let (_, s) = observed_pipeline(None);
    assert!(s.load_compute_overlap_ticks > 0, "{s:?}");
    assert!(s.store_compute_overlap_ticks > 0, "{s:?}");
    assert_eq!(
        (
            s.completed_load_chunks,
            s.completed_compute_chunks,
            s.completed_finalisations,
            s.completed_store_tiles
        ),
        (8, 8, 2, 2)
    );
    assert_eq!(
        (
            s.active_load_chunks,
            s.active_compute_chunks,
            s.active_finalisations,
            s.active_store_tiles
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        (
            s.cancelled_load_chunks,
            s.cancelled_compute_chunks,
            s.cancelled_finalisations,
            s.cancelled_store_tiles
        ),
        (0, 0, 0, 0)
    );
    assert!(!s.local_service_modelled);
}

#[test]
fn one_local_staging_service_charges_exact_bulk_ledger_and_stalls() {
    let unmodelled = observed_pipeline(None);
    let constrained = observed_pipeline(Some(1));
    // Two batches: input fills512 + operand reads512 + accumulator
    // readout512 + output writes256 + output drains256.
    assert_eq!(
        (
            unmodelled.1.local_read_bytes,
            unmodelled.1.local_write_bytes
        ),
        (1280, 768)
    );
    assert_eq!(
        (
            constrained.1.local_read_bytes,
            constrained.1.local_write_bytes
        ),
        (1280, 768)
    );
    assert!(constrained.1.local_service_modelled);
    assert!(
        constrained.0 > unmodelled.0,
        "{constrained:?} vs {unmodelled:?}"
    );
}

#[test]
fn cli_dump_stats_reports_pipeline_overlap_and_unmodelled_local_bandwidth() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path().join("platform.yaml");
    let t = temp.path().join("workload.yaml");
    std::fs::write(&p, platform(4096)).unwrap();
    std::fs::write(&t, workload("overlap", 2, 1, 1)).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .args([
            "--platform",
            p.to_str().unwrap(),
            "--timetable",
            t.to_str().unwrap(),
            "--stdout",
            "--dump-stats",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Block pipeline"), "{stdout}");
    assert!(stdout.contains("load_compute_overlap_ticks"), "{stdout}");
    assert!(stdout.contains("local_service_modelled: false"), "{stdout}");
}
