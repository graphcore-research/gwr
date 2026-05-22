// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

fn build_platform(
    engine: &gwr_engine::engine::Engine,
    clock: &gwr_engine::time::clock::Clock,
) -> Rc<Platform> {
    Rc::new(
        Platform::from_string(
            engine,
            clock,
            "
memory_maps:
  - name: default
    devices:
      - name: hbm0

processing_elements:
  - name: pe_add
    memory_map: default
    config:

  - name: pe_gemm
    memory_map: default
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1GiB
",
        )
        .unwrap(),
    )
}

fn build_timetable(engine: &gwr_engine::engine::Engine, platform: &Rc<Platform>) -> Timetable {
    Timetable::new(
        engine.top(),
        TimetableFile::from_string(
            "
nodes:
  - id: add_in0
    kind: tensor
    config:
      addr: 0x1000
      dtype: fp16
      shape: [10]

  - id: add_in1
    kind: tensor
    config:
      addr: 0x2000
      dtype: fp16
      shape: [10]

  - id: add0
    kind: compute
    op: add
    pe: pe_add
    input_views:
      -
      -
    output_views:
      -

  - id: add_out
    kind: tensor
    config:
      addr: 0x3000
      dtype: fp16
      shape: [10]

  - id: gemm_in0
    kind: tensor
    config:
      addr: 0x4000
      dtype: fp16
      shape: [2, 3]

  - id: gemm_in1
    kind: tensor
    config:
      addr: 0x5000
      dtype: fp16
      shape: [3, 4]

  - id: gemm0
    kind: compute
    op: gemm
    pe: pe_gemm
    input_views:
      -
      -
    output_views:
      -

  - id: gemm_out
    kind: tensor
    config:
      addr: 0x6000
      dtype: fp16
      shape: [2, 4]

edges:
  - from: add_in0
    to: add0
    kind: data
  - from: add_in1
    to: add0
    kind: data
  - from: add0
    to: add_out
    kind: data
  - from: gemm_in0
    to: gemm0
    kind: data
  - from: gemm_in1
    to: gemm0
    kind: data
  - from: gemm0
    to: gemm_out
    kind: data
",
        )
        .unwrap(),
        platform,
    )
    .unwrap()
}

#[test]
fn stats_include_machine_ops_for_compute_nodes() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = build_platform(&engine, &clock);
    let timetable = build_timetable(&engine, &platform);

    let stats = timetable.stats().unwrap();

    assert_eq!(stats.total_load_bytes, 76);
    assert_eq!(stats.total_store_bytes, 36);
    assert_eq!(stats.num_compute_nodes, 2);
    assert_eq!(stats.num_tensor_nodes, 6);
    assert_eq!(stats.num_memory_nodes, 0);
    assert_eq!(stats.machine_ops.adds, 26);
    assert_eq!(stats.machine_ops.muls, 24);
    assert_eq!(stats.machine_ops.compares, 0);
    assert_eq!(stats.total_machine_ops, 50);

    let compute_nodes = timetable.compute_node_analyses().unwrap();
    assert_eq!(compute_nodes.len(), 2);

    assert_eq!(compute_nodes[0].id, "add0");
    assert_eq!(compute_nodes[0].machine_ops.adds, 10);
    assert_eq!(compute_nodes[0].machine_ops.muls, 0);
    assert_eq!(compute_nodes[0].flops, 10);
    assert_eq!(compute_nodes[0].input_bytes, 40);
    assert_eq!(compute_nodes[0].output_bytes, 20);
    assert!(compute_nodes[0].predecessor_compute_node_indices.is_empty());

    assert_eq!(compute_nodes[1].id, "gemm0");
    assert_eq!(compute_nodes[1].machine_ops.adds, 16);
    assert_eq!(compute_nodes[1].machine_ops.muls, 24);
    assert_eq!(compute_nodes[1].flops, 40);
    assert_eq!(compute_nodes[1].input_bytes, 36);
    assert_eq!(compute_nodes[1].output_bytes, 16);
    assert!(compute_nodes[1].predecessor_compute_node_indices.is_empty());
}
