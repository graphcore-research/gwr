// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::Tracker;
use gwr_track::log::Level;
use gwr_track::test_helpers::TestTracker;

fn timetable_construction_error(source: &str, platform_source: &str) -> String {
    let tracker = Rc::new(TestTracker::new(1000, Level::Trace));
    let tracking: Tracker = tracker;
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, platform_source).unwrap());
    let graph = TimetableFile::from_string(source)
        .unwrap()
        .into_graph()
        .unwrap();
    let error = Timetable::new(engine.top(), graph, &platform)
        .err()
        .unwrap()
        .0;
    assert_eq!(platform.pe("pe0").unwrap().memory_traffic_bytes(), (0, 0));
    assert_eq!(platform.pe("pe0").unwrap().total_flops(), 0);
    assert_eq!(engine.time_now_ns(), 0.0);
    error
}

#[test]
fn unsupported_matrix_accumulator_is_rejected_before_runtime() {
    let platform = include_str!("../examples/compute_region_platform.yaml")
        .replace(
            "operation_classes:\n",
            "operation_classes:\n  matrix: { operations: [matrix_mac], arity: 2 }\n",
        )
        .replace("class: mul", "class: matrix, accumulator: bf16");
    let error = timetable_construction_error(
        r"
nodes:
 - { id: A, kind: tensor, config: { addr: 4096, dtype: fp32, shape: [2,2] } }
 - { id: B, kind: tensor, config: { addr: 8192, dtype: fp32, shape: [2,2] } }
 - { id: Y, kind: tensor, config: { addr: 12288, dtype: fp32, shape: [2,2] } }
 - id: gemm
   kind: compute
   pe: pe0
   program:
     inputs: [A,B]
     outputs: [Y]
     assignments:
      - output: Y
        axes: [m,n]
        expr:
          matrix_mac:
            accumulator: fp32
            over: [k]
            args:
              - read: { tensor: A, indices: [m, k] }
              - read: { tensor: B, indices: [k, n] }
edges:
 - { from: A, to: gemm.0, kind: data }
 - { from: B, to: gemm.1, kind: data }
 - { from: gemm, to: Y, kind: data }
",
        &platform,
    );
    assert!(error.contains("gemm") && error.contains("unsupported capability"));
}

#[test]
fn over_capacity_tile_is_rejected_before_runtime() {
    let error = timetable_construction_error(
        r"
nodes:
 - { id: A, kind: tensor, config: { addr: 4096, dtype: fp32, shape: [4096] } }
 - { id: Y, kind: tensor, config: { addr: 65536, dtype: fp32, shape: [4096] } }
 - id: copy
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [Y]
     assignments:
       - output: Y
         axes: [i]
         expr:
           read: { tensor: A, indices: [i] }
edges:
 - { from: A, to: copy, kind: data }
 - { from: copy, to: Y, kind: data }
",
        include_str!("../examples/compute_region_platform.yaml"),
    );
    assert!(error.contains("copy") && error.contains("SRAM"));
}

#[test]
fn unmapped_compute_output_is_rejected_before_runtime() {
    let error = timetable_construction_error(
        r"
nodes:
 - { id: A, kind: tensor, config: { addr: 4096, dtype: fp32, shape: [4] } }
 - { id: Y, kind: tensor, config: { addr: 2000000, dtype: fp32, shape: [4] } }
 - id: copy
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [Y]
     assignments:
       - output: Y
         axes: [i]
         expr:
           read: { tensor: A, indices: [i] }
edges:
 - { from: A, to: copy, kind: data }
 - { from: copy, to: Y, kind: data }
",
        include_str!("../examples/compute_region_platform.yaml"),
    );
    assert!(
        error.contains("copy") && error.contains("unmapped touched bytes"),
        "{error}"
    );
}

#[test]
fn unsupported_downstream_node_is_rejected_during_construction() {
    let platform = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: mul", "class: compare");
    let error = timetable_construction_error(include_str!("../examples/add_mul.yaml"), &platform);
    assert!(
        error.contains("multiply") && error.contains("unsupported capability"),
        "{error}"
    );
}
