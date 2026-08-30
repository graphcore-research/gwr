// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::process::Command;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::operators::HasShape;
use gwr_models::processing_element::task::{ComputeOp, Task};
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{NodeSection, TimetableFile};
use tempfile::tempdir;

const PLATFORM_YAML: &str = "
memory_maps:
  - name: default
    devices:
      - name: hbm0

processing_elements:
  - name: pe0
    memory_map: default
    config:
      lsu_access_bytes: 32
      sram_bytes: 64KiB

caches:
  - name: l1_0
    config:
      bw_bytes_per_cycle: 32
      line_size_bytes: 32
      delay_ticks: 4

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    capacity_bytes: 0x1000_0000

connections:
  - connect:
      - pe.pe0
      - cache.l1_0.dev
  - connect:
      - cache.l1_0.mem
      - mem.hbm0
";

const CUSTOM_TIMETABLE_YAML: &str = "
nodes:
  - id: input0
    kind: tensor
    config:
      addr: 0x1_0000_0000
      dtype: fp32
      shape: [2, 3]

  - id: input1
    kind: tensor
    config:
      addr: 0x1_0000_0400
      dtype: fp16
      shape: [4]

  - id: custom0
    kind: compute
    op:
      custom:
        name: fft_stage
        machine_ops:
          adds: 10
          muls: 20
          compares: 30
    pe: pe0
    input_views:
      -
      -
    output_views:
      -
      -

  - id: output0
    kind: tensor
    config:
      addr: 0x1_0000_0800
      dtype: fp32
      shape: [1]

  - id: output1
    kind: tensor
    config:
      addr: 0x1_0000_0c00
      dtype: int64
      shape: [2, 2]

edges:
  - from: input0
    to: custom0.0
    kind: data

  - from: input1
    to: custom0.1
    kind: data

  - from: custom0.0
    to: output0
    kind: data

  - from: custom0.1
    to: output1
    kind: data
";

fn create_timetable(yaml: &str) -> Timetable {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, PLATFORM_YAML).unwrap());
    let timetable_file = TimetableFile::from_string(yaml).unwrap();

    Timetable::new(engine.top(), timetable_file, &platform).unwrap()
}

#[test]
fn custom_compute_accepts_multiple_inputs_and_outputs() {
    let timetable = create_timetable(CUSTOM_TIMETABLE_YAML);
    let task = timetable.task_by_id(2).unwrap();

    let Task::ComputeTask { config } = task else {
        panic!("custom node did not produce a compute task");
    };

    assert_eq!(config.id, "custom0");
    assert_eq!(config.inputs.len(), 2);
    assert_eq!(config.outputs.len(), 2);
    assert_eq!(
        config.inputs[0].as_ref().unwrap().shape().dims(),
        &vec![2, 3]
    );
    assert_eq!(config.inputs[1].as_ref().unwrap().shape().dims(), &vec![4]);
    assert_eq!(config.outputs[0].as_ref().unwrap().shape().dims(), &vec![1]);
    assert_eq!(
        config.outputs[1].as_ref().unwrap().shape().dims(),
        &vec![2, 2]
    );

    let ComputeOp::Custom(operator) = config.op else {
        panic!("expected custom compute op");
    };
    assert_eq!(operator.name.as_deref(), Some("fft_stage"));
    assert_eq!(
        operator.machine_ops,
        MachineOpCounts {
            adds: 10,
            muls: 20,
            compares: 30,
        }
    );
}

#[test]
fn custom_compute_serializes_name() {
    let timetable_file = TimetableFile::from_string(CUSTOM_TIMETABLE_YAML).unwrap();
    let yaml = serde_yaml::to_string(&timetable_file).unwrap();

    assert!(
        yaml.contains("name: fft_stage"),
        "serialized timetable:\n{yaml}"
    );
}

#[test]
fn custom_compute_defaults_missing_machine_op_counts() {
    let timetable_file = TimetableFile::from_string(
        "
nodes:
  - id: custom0
    kind: compute
    op:
      custom:
        machine_ops:
          muls: 7
    pe: pe0
    input_views: []
    output_views: []

edges: []
",
    )
    .unwrap();

    let NodeSection::Compute { op, .. } = &timetable_file.nodes[0] else {
        panic!("expected compute node");
    };
    let ComputeOp::Custom(operator) = op else {
        panic!("expected custom compute op");
    };
    assert_eq!(operator.name, None);
    assert_eq!(
        operator.machine_ops,
        MachineOpCounts {
            adds: 0,
            muls: 7,
            compares: 0,
        }
    );
}

#[test]
fn custom_compute_name_is_used_in_activity_trace() {
    let (test_tracker, tracker) = gwr_track::test_init!(1000);
    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, PLATFORM_YAML).unwrap());
    let timetable_file = TimetableFile::from_string(CUSTOM_TIMETABLE_YAML).unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), timetable_file, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);

    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();

    let events = test_tracker.events();
    assert!(
        events
            .iter()
            .any(|event| event.contains("created group")
                && event.contains("pe0::fft_stage operation")),
        "missing named custom operation group in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("activity begin fft_stage compute")),
        "missing named custom compute activity in {events:#?}"
    );
}

#[test]
fn custom_compute_rejects_unknown_fields() {
    let err = TimetableFile::from_string(
        "
nodes:
  - id: custom0
    kind: compute
    op:
      custom:
        machine_ops:
          adds: 1
        latency: 2
    pe: pe0
    input_views: []
    output_views: []

edges: []
",
    )
    .unwrap_err();

    assert!(format!("{err}").contains("unknown field `latency`"));
}

#[test]
fn dump_stats_reports_custom_machine_ops() {
    let dir = tempdir().unwrap();
    let timetable_path = dir.path().join("timetable.yaml");
    fs::write(&timetable_path, CUSTOM_TIMETABLE_YAML).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .arg("--platform")
        .arg("../gwr-platform/examples/simple_pe_cache_mem.yaml")
        .arg("--timetable")
        .arg(&timetable_path)
        .arg("--stdout")
        .arg("--dump-stats")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "gwr-timetable failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("machine ops 60 total, 10 add, 20 mul, 30 compare"),
        "stdout:\n{stdout}"
    );
}
