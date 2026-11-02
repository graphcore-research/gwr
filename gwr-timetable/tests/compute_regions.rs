// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::SRAM_SLOT_ALIGNMENT_BYTES;
use gwr_models::processing_element::program::syntax::{ExprSpec, ProgramSpec};
use gwr_models::processing_element::task::Task;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{NodeSection, TimetableFile};
use gwr_track::log::Level;
use gwr_track::test_helpers::TestTracker;
use gwr_track::{Tracker, test_init};
use yaml_serde::{from_str, to_string};

const MATRIX_EPILOGUE_SRAM_BYTES: usize = 16 * 1024;
const MATRIX_EPILOGUE_TILE_BYTES: usize = 5192;

const LOCAL_TEMPORARY_PROGRAM: &str = r"
inputs: [A, B, C]
outputs: [Y]
temporaries: { T: { dtype: fp32 } }
assignments:
  - output: T
    axes: [i]
    expr:
      add:
        - read: { tensor: A, indices: [i] }
        - read: { tensor: B, indices: [i] }
  - output: Y
    axes: [i]
    expr:
      mul:
        - read: { tensor: T, indices: [i] }
        - read: { tensor: C, indices: [i] }
";

fn run_vector_timetable_with_sram(source: &str, sram_bytes: usize) -> (u64, Vec<String>) {
    let platform_source = include_str!("../examples/compute_region_platform.yaml").replace(
        "sram_bytes: 256",
        &format!("sram_bytes: {sram_bytes}\n      lsu_access_bytes: 16"),
    );
    let file = TimetableFile::from_string(source).unwrap();
    let simulation = run_timetable(&platform_source, file);
    let ops = simulation.platform.pe("pe0").unwrap().machine_ops();
    assert_eq!((ops.by_operation["add"], ops.by_operation["mul"]), (8, 8));
    (
        simulation.clock.tick_now().tick(),
        simulation.tracker.events(),
    )
}

fn run_timetable(platform_source: &str, file: TimetableFile) -> Simulation {
    let tracker = Rc::new(TestTracker::new(1000, Level::Trace));
    let tracking: Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, platform_source).unwrap());
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let planned = timetable.workload_totals().unwrap();
    assert_eq!(
        platform.pe("pe0").unwrap().memory_traffic_bytes(),
        (planned.read_bytes, planned.written_bytes),
        "planned boundary traffic must match completed execution",
    );
    Simulation {
        platform,
        tracker,
        clock,
        timetable,
    }
}

fn local_max_reduction_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [2, 3] } }
  - id: fused
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      temporaries: { T: { dtype: fp32 } }
      assignments:
        - output: T
          axes: [o, k]
          expr:
            add:
              - read: { tensor: A, indices: [o, k] }
              - read: { tensor: A, indices: [o, k] }
        - output: Y
          axes: [o]
          expr:
            reduce:
              op: max
              over: [k]
              value:
                read: { tensor: T, indices: [o, k] }
    tiling: { sizes: { o: 1 } }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [2] } }
edges:
  - { from: A, to: fused.0, kind: data }
  - { from: fused.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn affine_sum_window_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [3] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [3] } }
  - id: window
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      extents: { y: 3, k: 3 }
      assignments:
        - output: Y
          axes: [y]
          expr:
            reduce:
              op: sum
              over: [k]
              value:
                mul:
                  - read:
                      tensor: A
                      indices:
                        - affine:
                            terms: { y: 1, k: 1 }
                            offset: -1
                      bounds: exclude
                  - read: { tensor: B, indices: [k] }
    tiling: { sizes: { y: 1 } }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [3] } }
edges:
  - { from: A, to: window.0, kind: data }
  - { from: B, to: window.1, kind: data }
  - { from: window.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn unused_declared_input_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - { id: dependency, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
  - id: copy
    kind: compute
    pe: pe0
    program:
      inputs: [A, dependency]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            read: { tensor: A, indices: [i] }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4] } }
edges:
  - { from: A, to: copy.0, kind: data }
  - { from: dependency, to: copy.1, kind: data }
  - { from: copy.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn replicated_add_execution_units_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
  - id: first
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
  - id: second
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4] } }
  - { id: Z, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [4] } }
edges:
  - { from: A, to: first.0, kind: data }
  - { from: B, to: first.1, kind: data }
  - { from: first.0, to: Y, kind: data }
  - { from: A, to: second.0, kind: data }
  - { from: B, to: second.1, kind: data }
  - { from: second.0, to: Z, kind: data }
",
    )
    .unwrap()
}

fn fused_add_mul_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [8] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [8] } }
  - { id: C, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [8] } }
  - id: fused
    kind: compute
    pe: pe0
    program:
      inputs: [A, B, C]
      outputs: [Y]
      temporaries: { T: { dtype: fp32 } }
      assignments:
        - output: T
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
        - output: Y
          axes: [i]
          expr:
            mul:
              - read: { tensor: T, indices: [i] }
              - read: { tensor: C, indices: [i] }
    tiling: { sizes: { i: 4 } }
  - { id: Y, kind: tensor, config: { addr: 0x5000, dtype: fp32, shape: [8] } }
edges:
  - { from: A, to: fused.0, kind: data }
  - { from: B, to: fused.1, kind: data }
  - { from: C, to: fused.2, kind: data }
  - { from: fused.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn independent_assignments_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [8] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [8] } }
  - { id: C, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [8] } }
  - id: branches
    kind: compute
    pe: pe0
    program:
      inputs: [A, B, C]
      outputs: [Y, Z]
      assignments:
        - output: Y
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
        - output: Z
          axes: [i]
          expr:
            mul:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: C, indices: [i] }
  - { id: Y, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [8] } }
  - { id: Z, kind: tensor, config: { addr: 0x5000, dtype: fp32, shape: [8] } }
edges:
  - { from: A, to: branches.0, kind: data }
  - { from: B, to: branches.1, kind: data }
  - { from: C, to: branches.2, kind: data }
  - { from: branches.0, to: Y, kind: data }
  - { from: branches.1, to: Z, kind: data }
",
    )
    .unwrap()
}

fn dependent_and_slow_branch_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [8] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [8] } }
  - id: fused
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y, Z]
      temporaries: { T: { dtype: fp32 } }
      assignments:
        - output: T
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
        - output: Z
          axes: [i]
          expr:
            mul:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
        - output: Y
          axes: [i]
          expr:
            add:
              - read: { tensor: T, indices: [i] }
              - read: { tensor: B, indices: [i] }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [8] } }
  - { id: Z, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [8] } }
edges:
  - { from: A, to: fused.0, kind: data }
  - { from: B, to: fused.1, kind: data }
  - { from: fused.0, to: Y, kind: data }
  - { from: fused.1, to: Z, kind: data }
",
    )
    .unwrap()
}

fn chained_local_reductions_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [2, 3, 4] } }
  - id: nested
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      temporaries: { T: { dtype: fp32 } }
      assignments:
        - output: T
          axes: [i, j]
          expr:
            reduce:
              op: sum
              over: [k]
              value:
                read: { tensor: A, indices: [i, j, k] }
        - output: Y
          axes: [i]
          expr:
            reduce:
              op: sum
              over: [j]
              value:
                read: { tensor: T, indices: [i, j] }
    tiling: { sizes: { i: 1, j: 2, k: 2 } }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [2] } }
edges:
  - { from: A, to: nested.0, kind: data }
  - { from: nested.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn matrix_shaped_scalar_product_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [2, 3] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [3, 2] } }
  - id: gemm
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [m, n]
          expr:
            reduce:
              op: sum
              over: [k]
              value:
                mul:
                  - read: { tensor: A, indices: [m, k] }
                  - read: { tensor: B, indices: [k, n] }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [2, 2] } }
edges:
  - { from: A, to: gemm.0, kind: data }
  - { from: B, to: gemm.1, kind: data }
  - { from: gemm.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn vector_scalar_product_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [3] } }
  - id: sum_of_squares
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      assignments:
        - output: Y
          axes: []
          expr:
            reduce:
              op: sum
              over: [i]
              value:
                mul:
                  - read: { tensor: A, indices: [i] }
                  - read: { tensor: A, indices: [i] }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [] } }
edges:
  - { from: A, to: sum_of_squares.0, kind: data }
  - { from: sum_of_squares.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn chunked_matrix_reduction_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp16, shape: [2, 5] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp16, shape: [5, 3] } }
  - id: gemm
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [m, n]
          expr:
            matrix_mac:
              accumulator: fp16
              over: [k]
              args:
                - read: { tensor: A, indices: [m, k] }
                - read: { tensor: B, indices: [k, n] }
    tiling: { sizes: { m: 2, n: 3, k: 2 } }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp16, shape: [2, 3] } }
edges:
  - { from: A, to: gemm.0, kind: data }
  - { from: B, to: gemm.1, kind: data }
  - { from: gemm.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn chunked_window_max_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [5] } }
  - id: pool
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y, I]
      extents: { o: 3, k: 3 }
      assignments:
        - output: Y
          indices_output: I
          axes: [o]
          expr:
            reduce:
              op: max
              over: [k]
              value:
                read:
                  tensor: A
                  indices:
                    - affine:
                        terms: { o: 2, k: 1 }
                        offset: -1
                  bounds: exclude
    tiling: { sizes: { o: 1, k: 1 } }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [3] } }
  - { id: I, kind: tensor, config: { addr: 0x3000, dtype: int64, shape: [3] } }
edges:
  - { from: A, to: pool.0, kind: data }
  - { from: pool.0, to: Y, kind: data }
  - { from: pool.1, to: I, kind: data }
",
    )
    .unwrap()
}

fn dilated_window_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [5] } }
  - id: pool
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      extents: { o: 1, k: 3 }
      assignments:
        - output: Y
          axes: [o]
          expr:
            reduce:
              op: max
              over: [k]
              value:
                read:
                  tensor: A
                  indices:
                    - affine:
                        terms: { k: 2 }
                        offset: 0
                  bounds: exclude
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [1] } }
edges:
  - { from: A, to: pool.0, kind: data }
  - { from: pool.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn broadcast_max_reduction_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1] } }
  - id: pool
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      extents: { k: 3 }
      assignments:
        - output: Y
          axes: []
          expr:
            reduce:
              op: max
              over: [k]
              value:
                read: { tensor: A, indices: [k], bounds: exclude }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [] } }
edges:
  - { from: A, to: pool.0, kind: data }
  - { from: pool.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn matrix_epilogue_platform(sram_bytes: usize) -> String {
    format!(
        r"
memory_maps:
  - name: mm
    devices: [mem]
operation_classes:
  matrix: {{ operations: [matrix_mac], arity: 2, flops_per_op: 2 }}
  add: {{ operations: [add], arity: 2 }}
  compare: {{ operations: [compare, maximum], arity: 2 }}
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: {sram_bytes}
      compute:
        execution_units:
          matrix:
            capabilities: [{{ class: matrix, accumulator: fp32, input_dtypes: [[fp16, fp16]], ops_per_tick: 1024 }}]
          vector:
            capabilities:
              - {{ class: add, input_dtypes: [[fp32, fp32]], ops_per_tick: 1024 }}
              - {{ class: compare, input_dtypes: [[fp32, fp32]], ops_per_tick: 1024 }}
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: {{ capacity_bytes: 1MiB }}
connections:
  - connect: [pe.pe0, mem.mem]
"
    )
}

fn matrix_epilogue_timetable() -> TimetableFile {
    TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp16, shape: [64, 256] } }
  - { id: B, kind: tensor, config: { addr: 0x9000, dtype: fp16, shape: [256, 64] } }
  - { id: bias, kind: tensor, config: { addr: 0x11000, dtype: fp32, shape: [64] } }
  - { id: floor, kind: tensor, config: { addr: 0x12000, dtype: fp32, shape: [1] } }
  - id: fused
    kind: compute
    pe: pe0
    program:
      inputs: [A, B, bias, floor]
      outputs: [Y]
      temporaries: { product: { dtype: fp32 }, biased: { dtype: fp32 } }
      assignments:
        - output: product
          axes: [m, n]
          expr:
            matrix_mac:
              accumulator: fp32
              over: [k]
              args:
                - read: { tensor: A, indices: [m, k] }
                - read: { tensor: B, indices: [k, n] }
        - output: biased
          axes: [m, n]
          expr:
            add:
              - read: { tensor: product, indices: [m, n] }
              - read: { tensor: bias, indices: [n] }
        - output: Y
          axes: [m, n]
          expr:
            maximum:
              - read: { tensor: biased, indices: [m, n] }
              - read: { tensor: floor, indices: [0] }
    tiling: { sizes: { m: 16, n: 16, k: 32 } }
  - { id: Y, kind: tensor, config: { addr: 0x13000, dtype: fp32, shape: [64, 64] } }
edges:
  - { from: A, to: fused.0, kind: data }
  - { from: B, to: fused.1, kind: data }
  - { from: bias, to: fused.2, kind: data }
  - { from: floor, to: fused.3, kind: data }
  - { from: fused.0, to: Y, kind: data }
",
    )
    .unwrap()
}

fn assert_matrix_epilogue_operation_counts(simulation: &Simulation) {
    let ops = simulation.platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(
        (
            ops.by_operation["matrix_mac"],
            ops.by_operation["add"],
            ops.by_operation["maximum"]
        ),
        (1_048_576, 4096, 4096)
    );
}

struct Simulation {
    platform: Rc<Platform>,
    tracker: Rc<TestTracker>,
    clock: Clock,
    timetable: Rc<Timetable>,
}

fn memory_request_bytes(events: &[String], kind: &str) -> usize {
    events
        .iter()
        .filter(|event| {
            event.contains("created object")
                && event.contains("MemoryAccess:")
                && event.contains(kind)
        })
        .map(|event| {
            event
                .split("MemoryAccess: ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .split('@')
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap()
        })
        .sum()
}

fn assert_rejected_before_loading(platform_source: &str, file: TimetableFile, expected: &str) {
    let tracker = Rc::new(TestTracker::new(1000, Level::Trace));
    let tracking: Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, platform_source).unwrap());
    let error = Timetable::new(engine.top(), file.into_graph().unwrap(), &platform)
        .err()
        .unwrap();
    assert!(error.0.contains(expected), "{error:?}");
    assert!(
        !tracker
            .events()
            .iter()
            .any(|event| event.contains("MemoryAccess:"))
    );
}

fn sum_reduction_timetable(reduction_body: &str, uses_local_temporary: bool) -> String {
    let reduction_body = reduction_body.trim().replace('\n', "\n                ");
    let uses_b = uses_local_temporary || reduction_body.contains("tensor: B");
    let inputs = if uses_b { "[A, B]" } else { "[A]" };
    let b_edge = if uses_b {
        "  - { from: B, to: reduce.1, kind: data }"
    } else {
        ""
    };
    let temporary_decl = if uses_local_temporary {
        "      temporaries: { T: { dtype: fp32 } }"
    } else {
        ""
    };
    let prelude = if uses_local_temporary {
        r"
        - output: T
          axes: [i, k]
          expr:
            add:
              - read: { tensor: A, indices: [i, k] }
              - read: { tensor: B, indices: [i, k] }
"
    } else {
        ""
    };
    let tiling = "{ i: 2, k: 2 }";
    format!(
        r"
nodes:
  - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: fp32, shape: [4, 3] }} }}
  - {{ id: B, kind: tensor, config: {{ addr: 0x2000, dtype: fp32, shape: [4, 3] }} }}
  - id: reduce
    kind: compute
    pe: pe0
    program:
      inputs: {inputs}
      outputs: [Y]
{temporary_decl}
      assignments:
{prelude}
        - output: Y
          axes: [i]
          expr:
            reduce:
              op: sum
              over: [k]
              value:
                {reduction_body}
    tiling: {{ sizes: {tiling} }}
  - {{ id: Y, kind: tensor, config: {{ addr: 0x3000, dtype: fp32, shape: [4] }} }}
edges:
  - {{ from: A, to: reduce.0, kind: data }}
{b_edge}
  - {{ from: reduce.0, to: Y, kind: data }}
"
    )
}

fn fanout_timetable(same_pe: bool) -> (Timetable, [usize; 3]) {
    let tracker = Rc::new(TestTracker::new(1000, Level::Trace));
    let tracking: Tracker = tracker;
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, &fanout_platform()).unwrap());
    let graph = TimetableFile::from_string(&fanout_timetable_yaml(same_pe))
        .unwrap()
        .into_graph()
        .unwrap();
    let index = |name: &str| {
        graph
            .nodes()
            .iter()
            .position(|node| node.id() == name)
            .unwrap()
    };
    let nodes = [index("produce"), index("consume1"), index("consume2")];
    (
        Timetable::new(engine.top(), graph, &platform).unwrap(),
        nodes,
    )
}

fn assert_compute_ready(timetable: &Timetable, pe: &str, node: usize) {
    assert!(timetable.ready_task_indices(pe).unwrap().1.contains(&node));
}

fn assert_compute_not_ready(timetable: &Timetable, pe: &str, node: usize) {
    assert!(!timetable.ready_task_indices(pe).unwrap().1.contains(&node));
}

fn fanout_platform() -> String {
    include_str!("../examples/compute_region_platform.yaml").replace(
        "memories:\n",
        "  - name: pe1
    memory_map: mm
    config:
      sram_bytes: 256
      compute:
        execution_units:
          vector_add:
            capabilities: [{ class: add, ops_per_tick: 1 }]
          vector_mul:
            capabilities: [{ class: mul, ops_per_tick: 1 }]
  - name: pe2
    memory_map: mm
    config:
      sram_bytes: 256
      compute:
        execution_units:
          vector_add:
            capabilities: [{ class: add, ops_per_tick: 1 }]
          vector_mul:
            capabilities: [{ class: mul, ops_per_tick: 1 }]
memories:
",
    )
}

fn fanout_timetable_yaml(same_pe: bool) -> String {
    let source = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
  - id: produce
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [T]
      assignments:
        - output: T
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
    tiling: { sizes: { i: 2 } }
  - { id: T, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4] } }
  - id: consume1
    kind: compute
    pe: pe1
    program:
      inputs: [T0, T1]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            add:
              - read: { tensor: T0, indices: [i] }
              - read: { tensor: T1, indices: [i] }
  - { id: Y, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [4] } }
  - id: consume2
    kind: compute
    pe: pe2
    program:
      inputs: [T0, T1]
      outputs: [Z]
      assignments:
        - output: Z
          axes: [i]
          expr:
            add:
              - read: { tensor: T0, indices: [i] }
              - read: { tensor: T1, indices: [i] }
  - { id: Z, kind: tensor, config: { addr: 0x5000, dtype: fp32, shape: [4] } }
edges:
  - { from: A, to: produce.0, kind: data }
  - { from: B, to: produce.1, kind: data }
  - { from: produce.0, to: T, kind: data }
  - { from: T, to: consume1.0, kind: data }
  - { from: T, to: consume1.1, kind: data }
  - { from: consume1.0, to: Y, kind: data }
  - { from: T, to: consume2.0, kind: data }
  - { from: T, to: consume2.1, kind: data }
  - { from: consume2.0, to: Z, kind: data }
";
    if same_pe {
        source
            .replace("pe: pe1", "pe: pe0")
            .replace("pe: pe2", "pe: pe0")
    } else {
        source.to_owned()
    }
}

fn chunked_matrix_platform() -> &'static str {
    r"
memory_maps:
  - name: mm
    devices: [mem]
operation_classes:
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: 256
      compute:
        execution_units:
          matrix:
            capabilities: [{ class: matrix, accumulator: fp16, input_dtypes: [[fp16, fp16]], ops_per_tick: 10 }]
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: { capacity_bytes: 1MiB }
connections:
  - connect: [pe.pe0, mem.mem]
"
}

fn window_max_platform() -> &'static str {
    r"
memory_maps:
  - name: mm
    devices: [mem]
operation_classes:
  compare: { operations: [compare, maximum], arity: 2 }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: 256
      compute:
        execution_units:
          compare:
            capabilities: [{ class: compare, input_dtypes: [[fp32, fp32]], ops_per_tick: 4 }]
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: { capacity_bytes: 1MiB }
connections:
  - connect: [pe.pe0, mem.mem]
"
}

#[test]
fn fused_temporary_reduction_avoids_intermediate_boundary_traffic() {
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: mul", "class: compare");
    let file = local_max_reduction_timetable();
    let simulation = run_timetable(&platform_source, file);
    assert_eq!(
        simulation
            .platform
            .pe("pe0")
            .unwrap()
            .machine_ops()
            .by_operation["compare"],
        4
    );
    assert_eq!(
        simulation
            .platform
            .pe("pe0")
            .unwrap()
            .memory_traffic_bytes(),
        (24, 8)
    );
}

#[test]
fn affine_sum_reduction_transfers_only_valid_window_elements() {
    let file = affine_sum_window_timetable();
    let simulation = run_timetable(
        include_str!("../examples/compute_region_platform.yaml"),
        file,
    );
    assert_eq!(
        simulation
            .platform
            .pe("pe0")
            .unwrap()
            .memory_traffic_bytes(),
        (64, 12)
    );
}

#[test]
fn unused_declared_input_is_not_loaded_or_reserved_in_sram() {
    let file = unused_declared_input_timetable();
    let simulation = run_timetable(
        include_str!("../examples/compute_region_platform.yaml"),
        file,
    );
    assert_eq!(
        simulation
            .platform
            .pe("pe0")
            .unwrap()
            .memory_traffic_bytes(),
        (16, 16)
    );
    assert_eq!(simulation.platform.pe("pe0").unwrap().peak_sram_bytes(), 32);
}

#[test]
fn replicated_vector_execution_unit_configuration_executes_both_add_nodes() {
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: mul", "class: add");
    let file = replicated_add_execution_units_timetable();
    let simulation = run_timetable(&platform_source, file);
    let events = simulation.tracker.events();
    assert!(
        events
            .iter()
            .any(|event| event.contains("lane::compute::vector_add::0"))
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("lane::compute::vector_mul::0"))
    );
    assert_eq!(
        simulation
            .platform
            .pe("pe0")
            .unwrap()
            .machine_ops()
            .by_operation["add"],
        8
    );
}

#[test]
fn parses_a_program_with_a_local_temporary() {
    let program: ProgramSpec = from_str(LOCAL_TEMPORARY_PROGRAM).unwrap();
    assert_eq!(program.inputs, ["A", "B", "C"]);
    assert_eq!(program.outputs, ["Y"]);
    assert!(program.temporaries.contains_key("T"));
    assert_eq!(program.assignments.len(), 2);
    assert_eq!(program.assignments[0].output.as_deref(), Some("T"));
    assert!(matches!(&program.assignments[0].expr, ExprSpec::Call { op, .. } if op == "add"));
    assert!(matches!(&program.assignments[1].expr, ExprSpec::Call { op, .. } if op == "mul"));
}

#[test]
fn program_serialization_round_trip_preserves_assignments() {
    let program: ProgramSpec = from_str(LOCAL_TEMPORARY_PROGRAM).unwrap();
    let round_trip: ProgramSpec = from_str(&to_string(&program).unwrap()).unwrap();
    assert_eq!(round_trip, program);
}

#[test]
fn rejects_ambiguous_or_incomplete_expressions() {
    for expression in [
        r"
add:
  wrong: []
",
        r"
read: { tensor: A, indices: [i] }
mul:
  - read: { tensor: A, indices: [i] }
  - read: { tensor: A, indices: [i] }
",
        r"
reduce:
  op: sum
  over: [i]
",
    ] {
        let expression = expression.trim().replace('\n', "\n      ");
        let source = format!(
            r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      {expression}
"
        );
        assert!(
            from_str::<ProgramSpec>(&source).is_err(),
            "accepted {expression}"
        );
    }
}

#[test]
fn timetable_compute_node_carries_its_program_and_tiling() {
    let timetable = TimetableFile::from_string(
        r"
nodes:
  - id: A
    kind: tensor
    config: { addr: 0x1000, dtype: fp32, shape: [8] }
  - id: B
    kind: tensor
    config: { addr: 0x2000, dtype: fp32, shape: [8] }
  - id: Y
    kind: tensor
    config: { addr: 0x3000, dtype: fp32, shape: [8] }
  - id: add
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
    tiling: { sizes: { i: 4 } }
edges:
  - { from: A, to: add.0, kind: data }
  - { from: B, to: add.1, kind: data }
  - { from: add.0, to: Y, kind: data }
",
    )
    .unwrap();
    let NodeSection::Compute {
        program, tiling, ..
    } = &timetable.nodes[3]
    else {
        panic!("expected compute");
    };
    assert_eq!(program.inputs, ["A", "B"]);
    assert_eq!(tiling.as_ref().unwrap().sizes["i"], 4);
}

#[test]
fn fused_tiled_add_and_multiply_report_expected_operation_counts() {
    let file = fused_add_mul_timetable();
    let simulation = run_timetable(
        include_str!("../examples/compute_region_platform.yaml"),
        file,
    );
    let ops = simulation.platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(ops.by_operation.get("add"), Some(&8));
    assert_eq!(ops.by_operation.get("mul"), Some(&8));
}

#[test]
fn independent_assignments_overlap_on_separate_execution_units() {
    let file = independent_assignments_timetable();
    let simulation = run_timetable(
        include_str!("../examples/compute_region_platform.yaml"),
        file,
    );
    let events = simulation.tracker.events();
    let position = |marker: &str| {
        events
            .iter()
            .position(|event| event.contains(marker))
            .unwrap_or_else(|| panic!("missing {marker}"))
    };
    assert!(position("assignment Z begin") < position("assignment Y end"));
    assert!(position("assignment Y begin") < position("assignment Z end"));
    // The third input read is still outstanding when the first assignment
    // starts; it does not consume that input.
    let c_read_activity_id = events
        .iter()
        .find(|event| event.contains("activity begin branches C read"))
        .unwrap()
        .split(':')
        .next()
        .unwrap();
    assert!(
        position("assignment Y begin") < position(&format!("{c_read_activity_id}: activity end"))
    );
}

#[test]
fn dependent_assignment_starts_before_unrelated_slow_branch_finishes() {
    let platform_source = include_str!("../examples/compute_region_platform.yaml").replace(
        "class: mul, input_dtypes: [[fp32, fp32]], ops_per_tick: 1",
        "class: mul, input_dtypes: [[fp32, fp32]], ops_per_tick: 0.25",
    );
    let file = dependent_and_slow_branch_timetable();
    let simulation = run_timetable(&platform_source, file);
    let events = simulation.tracker.events();
    let position = |marker: &str| {
        events
            .iter()
            .position(|event| event.contains(marker))
            .unwrap()
    };
    assert!(position("assignment Y begin") < position("assignment Z end"));
}

#[test]
fn tiled_sum_of_reads_and_additions_counts_each_operation() {
    for (reduction_body, expected_adds, uses_local_temporary) in [
        (
            r"
read: { tensor: A, indices: [i, k] }
",
            8,
            false,
        ),
        (
            r"
add:
  - read: { tensor: A, indices: [i, k] }
  - read: { tensor: B, indices: [i, k] }
",
            20,
            false,
        ),
        (
            r"
read: { tensor: T, indices: [i, k] }
",
            20,
            true,
        ),
    ] {
        let source = sum_reduction_timetable(reduction_body, uses_local_temporary);
        let file = TimetableFile::from_string(&source).unwrap();
        let simulation = run_timetable(
            include_str!("../examples/compute_region_platform.yaml"),
            file,
        );
        let ops = simulation.platform.pe("pe0").unwrap().machine_ops();
        assert_eq!(
            (
                ops.by_operation.get("add").copied().unwrap_or(0),
                ops.by_operation.get("mul").copied().unwrap_or(0)
            ),
            (expected_adds, 0),
            "{reduction_body}"
        );
    }
}

#[test]
fn chained_local_reductions_reserve_full_intermediate_axis_in_sram() {
    let file = chained_local_reductions_timetable();
    let simulation = run_timetable(
        include_str!("../examples/compute_region_platform.yaml"),
        file,
    );
    assert_eq!(
        simulation
            .platform
            .pe("pe0")
            .unwrap()
            .machine_ops()
            .by_operation["add"],
        22
    );
    // Retain all three FP32 entries of T for one i (12 bytes, aligned to 16)
    // while streaming a 2-by-2 FP32 input chunk (16 bytes).
    let retained_t_bytes = (3usize * 4).next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);
    let input_chunk_bytes = 2 * 2 * 4;
    assert_eq!(
        simulation.platform.pe("pe0").unwrap().peak_sram_bytes(),
        retained_t_bytes + input_chunk_bytes
    );
}

#[test]
fn materialized_intermediate_waits_for_all_tiles_and_finishes_later() {
    let fused_source = include_str!("../examples/fused_add_mul.yaml");
    let materialized_source = include_str!("../examples/add_mul.yaml");
    let (fused_ticks, fused_events) = run_vector_timetable_with_sram(fused_source, 256);
    let (materialized_ticks, materialized_events) =
        run_vector_timetable_with_sram(materialized_source, 256);
    assert_eq!(memory_request_bytes(&fused_events, ""), 128);
    assert_eq!(memory_request_bytes(&materialized_events, ""), 192);
    // Each 16-byte boundary request has an eight-tick memory response. The
    // LSU issues requests serially, while independent loads and execution unit
    // work overlap once their own dependencies are satisfied.
    assert_eq!((fused_ticks, materialized_ticks), (33, 58));
    let position = |events: &[String], marker: &str| {
        events
            .iter()
            .position(|event| event.contains(marker))
            .unwrap_or_else(|| panic!("missing {marker}"))
    };
    assert!(
        position(&fused_events, "fused tile {\"i\": 0..4} assignment Y begin")
            < position(&fused_events, "fused tile {\"i\": 4..8} assignment T end")
    );
    assert!(
        position(&materialized_events, "task3: completed")
            < position(
                &materialized_events,
                "multiply tile {\"i\": 0..4} assignment Y begin"
            )
    );
    assert!(
        fused_ticks < materialized_ticks,
        "fused={fused_ticks}, materialized={materialized_ticks}"
    );
}

#[test]
fn materialized_fanout_waits_for_tensor_completion_on_same_or_other_pes() {
    for same_pe in [false, true] {
        let (timetable, [producer, first, second]) = fanout_timetable(same_pe);
        let first_pe = if same_pe { "pe0" } else { "pe1" };
        let second_pe = if same_pe { "pe0" } else { "pe2" };

        assert_compute_not_ready(&timetable, first_pe, first);
        assert_compute_not_ready(&timetable, second_pe, second);
        timetable.set_task_active(producer).unwrap();
        assert_compute_not_ready(&timetable, first_pe, first);
        timetable.set_task_completed(producer).unwrap();
        assert_compute_ready(&timetable, first_pe, first);
        assert_compute_ready(&timetable, second_pe, second);
    }
}

#[test]
fn shared_sram_admits_two_fused_tiles_only_when_both_arenas_fit() {
    let source = include_str!("../examples/fused_add_mul.yaml");
    // Three 16-byte inputs, one 16-byte output, and a 16-byte local
    // intermediate make each tile's aligned arena exactly 80 bytes.
    let (overlap_ticks, overlap_events) = run_vector_timetable_with_sram(source, 160);
    let (serialized_ticks, serialized_events) = run_vector_timetable_with_sram(source, 80);
    let second_tile_read_position = |events: &[String]| {
        events
            .iter()
            .position(|event| event.contains("MemoryAccess: ReadRequest: 16@1010"))
            .expect("second tile A read")
    };
    let first_tile_store_position = |events: &[String]| {
        events
            .iter()
            .position(|event| event.contains("MemoryAccess: WriteNonPostedRequest: 16@5000"))
            .expect("first tile Y store")
    };
    assert!(
        second_tile_read_position(&overlap_events) < first_tile_store_position(&overlap_events)
    );
    assert!(
        second_tile_read_position(&serialized_events)
            > first_tile_store_position(&serialized_events)
    );
    assert!(overlap_ticks < serialized_ticks);
}

#[test]
fn a_tile_one_byte_larger_than_sram_fails_before_loading() {
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("sram_bytes: 256", "sram_bytes: 79");
    let file = TimetableFile::from_string(include_str!("../examples/fused_add_mul.yaml")).unwrap();
    assert_rejected_before_loading(&platform_source, file, "requires 80 SRAM bytes");
}

#[test]
fn missing_typed_execution_unit_fails_before_loading() {
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: mul", "class: compare");
    let file = TimetableFile::from_string(include_str!("../examples/fused_add_mul.yaml")).unwrap();
    assert_rejected_before_loading(&platform_source, file, "unsupported capability");
}

#[test]
fn compute_completion_tick_overflow_is_an_error() {
    let tracker = Rc::new(TestTracker::new(1000, Level::Trace));
    let tracking: Tracker = tracker;
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_yaml = include_str!("../examples/compute_region_platform.yaml")
        .replace("ops_per_tick: 1", "ops_per_tick: 5e-19");
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml).unwrap());
    let graph = TimetableFile::from_string(include_str!("../examples/fused_add_mul.yaml"))
        .unwrap()
        .into_graph()
        .unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    let error = engine.run().unwrap_err();
    assert!(
        error.0.contains("compute completion tick overflows"),
        "{error:?}"
    );
}

#[test]
fn matrix_shaped_sum_of_products_uses_scalar_operations() {
    let file = matrix_shaped_scalar_product_timetable();
    let simulation = run_timetable(
        include_str!("../examples/compute_region_platform.yaml"),
        file,
    );
    let counts = simulation.platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(counts.by_operation["mul"], 12);
    assert_eq!(counts.by_operation["add"], 8);
    assert!(!counts.by_operation.contains_key("matrix_mac"));
}

#[test]
fn non_matrix_sum_of_products_still_uses_scalar_operations() {
    let file = vector_scalar_product_timetable();
    let simulation = run_timetable(
        include_str!("../examples/compute_region_platform.yaml"),
        file,
    );
    let counts = simulation.platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(counts.by_operation["mul"], 3);
    assert_eq!(counts.by_operation["add"], 2);
    assert!(!counts.by_operation.contains_key("matrix_mac"));
}

#[test]
fn tiled_matrix_reduction_keeps_partials_local_and_counts_macs() {
    let file = chunked_matrix_reduction_timetable();
    let simulation = run_timetable(chunked_matrix_platform(), file);
    let ops = simulation.platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(ops.by_operation["matrix_mac"], 30);
    assert!(!ops.by_operation.contains_key("add"));
    assert_eq!(
        simulation.timetable.machine_operations_executed().unwrap(),
        30
    );
    assert_eq!(simulation.clock.tick_now().tick(), 78);
    let events = simulation.tracker.events();
    assert_eq!(memory_request_bytes(&events, "ReadRequest"), 288);
    assert_eq!(memory_request_bytes(&events, "WriteNonPostedRequest"), 32);
}

#[test]
fn tiled_window_max_counts_valid_elements_and_stores_index_result() {
    let file = chunked_window_max_timetable();
    let simulation = run_timetable(window_max_platform(), file);
    let ops = simulation.platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(ops.by_operation["compare"], 4);
    let events = simulation.tracker.events();
    assert_eq!(memory_request_bytes(&events, "ReadRequest"), 224);
    assert_eq!(memory_request_bytes(&events, "WriteNonPostedRequest"), 192);
}

#[test]
fn ordinary_sparse_affine_reads_load_only_selected_elements() {
    let sparse = "{ affine: { terms: { i: 2 }, offset: 0 } }";
    for (second_index, read_bytes, sram_bytes) in [
        (None, 16, 32),
        (Some(sparse), 16, 48),
        (Some("{ affine: { terms: { i: 1 }, offset: 0 } }"), 32, 64),
    ] {
        for use_call in [false, true] {
            let mut engine = Engine::default();
            let clock = engine.default_clock();
            let platform_source = include_str!("../examples/compute_region_platform.yaml").replace(
                "sram_bytes: 256",
                &format!("sram_bytes: {sram_bytes}\n      lsu_access_bytes: 1"),
            );
            let platform =
                Rc::new(Platform::from_string(&engine, &clock, &platform_source).unwrap());
            let expression = |index: &str| {
                let read = format!(
                    r"read:
  tensor: A
  indices: [{index}]"
                );
                if use_call {
                    let read = read.replace('\n', "\n    ");
                    format!(
                        r"add:
  - {read}
  - {read}"
                    )
                } else {
                    read
                }
            };
            let second_assignment = second_index.map_or(String::new(), |index| {
                format!(
                    r"
      - output: Z
        axes: [i]
        expr:
          {}
",
                    expression(index).replace('\n', "\n          ")
                )
            });
            let outputs = if second_index.is_some() {
                "[Y, Z]"
            } else {
                "[Y]"
            };
            let source = format!(
                r"
nodes:
 - {{ id: A, kind: tensor, config: {{ addr: 4096, dtype: fp32, shape: [7] }} }}
 - {{ id: Y, kind: tensor, config: {{ addr: 8192, dtype: fp32, shape: [4] }} }}
 - {{ id: Z, kind: tensor, config: {{ addr: 12288, dtype: fp32, shape: [4] }} }}
 - id: gather
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: {outputs}
     extents: {{ i: 4 }}
     assignments:
      - output: Y
        axes: [i]
        expr:
          {}{second_assignment}
edges:
 - {{ from: A, to: gather, kind: data }}
 - {{ from: gather.0, to: Y, kind: data }}
{}
",
                expression(sparse).replace('\n', "\n          "),
                if second_index.is_some() {
                    " - { from: gather.1, to: Z, kind: data }"
                } else {
                    ""
                }
            );
            let graph = TimetableFile::from_string(&source)
                .unwrap()
                .into_graph()
                .unwrap();
            let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
            let written_bytes = if second_index.is_some() { 32 } else { 16 };
            assert_eq!(timetable.workload_totals().unwrap().read_bytes, read_bytes);
            let dispatcher: Rc<dyn Dispatch> = timetable.clone();
            platform.attach_dispatcher(&dispatcher);
            engine.run().unwrap();
            timetable.check_tasks_complete().unwrap();
            let pe = platform.pe("pe0").unwrap();
            assert_eq!(pe.peak_sram_bytes(), sram_bytes);
            assert_eq!(pe.memory_traffic_bytes(), (read_bytes, written_bytes));
            assert_eq!(platform.memory("mem").unwrap().bytes_read(), read_bytes);
        }
    }
}

#[test]
fn index_only_scalar_reduction_sizes_buffers_using_value_dtype() {
    for index_dtype in ["int8", "int64"] {
        for sram_bytes in [32, 40] {
            let mut engine = Engine::default();
            let clock = engine.default_clock();
            let platform_source = include_str!("../examples/compute_region_platform.yaml")
                .replace("sram_bytes: 256", &format!("sram_bytes: {sram_bytes}"))
                .replace("class: add", "class: compare")
                .replace("operations: [mul], arity: 2", "operations: [sin], arity: 1")
                .replace(
                    "class: mul, input_dtypes: [[fp32, fp32]]",
                    "class: mul, input_dtypes: [[fp32]]",
                );
            let platform =
                Rc::new(Platform::from_string(&engine, &clock, &platform_source).unwrap());
            let source = format!(
                r"
nodes:
 - {{ id: A, kind: tensor, config: {{ addr: 4096, dtype: fp32, shape: [1, 4] }} }}
 - {{ id: I, kind: tensor, config: {{ addr: 8192, dtype: {index_dtype}, shape: [1] }} }}
 - id: argmax
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [I]
     assignments:
      - indices_output: I
        axes: [m]
        expr:
          reduce:
            op: max
            over: [k]
            value:
              sin:
                - read: {{ tensor: A, indices: [m, k] }}
edges:
 - {{ from: A, to: argmax, kind: data }}
 - {{ from: argmax, to: I, kind: data }}
"
            );
            let graph = TimetableFile::from_string(&source)
                .unwrap()
                .into_graph()
                .unwrap();
            let timetable = Timetable::new(engine.top(), graph, &platform);
            if sram_bytes == 32 {
                let error = timetable.err().unwrap();
                assert!(error.0.contains("tile requires 40 SRAM bytes"), "{error}");
            } else {
                let timetable = Rc::new(timetable.unwrap());
                let dispatcher: Rc<dyn Dispatch> = timetable.clone();
                platform.attach_dispatcher(&dispatcher);
                engine.run().unwrap();
                timetable.check_tasks_complete().unwrap();
                let pe = platform.pe("pe0").unwrap();
                assert_eq!(pe.peak_sram_bytes(), 40);
                assert_eq!(pe.machine_ops().by_operation["sin"], 4);
                assert_eq!(pe.machine_ops().by_operation["compare"], 3);
            }
        }
    }
}

#[test]
fn chunked_index_only_reduction_reserves_winning_value_sram() {
    let source = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1, 4] } }
  - id: argmax
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [I]
      assignments:
        - indices_output: I
          axes: [i]
          expr:
            reduce:
              op: max
              over: [k]
              value:
                read: { tensor: A, indices: [i, k] }
    tiling: { sizes: { k: 2 } }
  - { id: I, kind: tensor, config: { addr: 0x2000, dtype: int64, shape: [1] } }
edges:
  - { from: A, to: argmax.0, kind: data }
  - { from: argmax.0, to: I, kind: data }
";
    let run = |sram_bytes: usize| {
        let (_test_tracker, tracker) = test_init!(1000);
        let mut engine = Engine::new(&tracker);
        let clock = engine.default_clock();
        let platform_yaml = format!(
            r"
memory_maps:
  - name: mm
    devices: [mem]
operation_classes:
  compare: {{ operations: [compare, maximum], arity: 2 }}
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: {sram_bytes}
      compute:
        execution_units:
          vector:
            capabilities: [{{ class: compare, ops_per_tick: 4 }}]
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: {{ capacity_bytes: 1MiB }}
connections:
  - connect: [pe.pe0, mem.mem]
"
        );
        let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml).unwrap());
        let file = TimetableFile::from_string(source).unwrap();
        let timetable = Rc::new(Timetable::new(
            engine.top(),
            file.into_graph().unwrap(),
            &platform,
        )?);
        let dispatcher: Rc<dyn Dispatch> = timetable.clone();
        platform.attach_dispatcher(&dispatcher);
        engine.run()
    };
    let error = run(16).unwrap_err();
    assert!(error.0.contains("tile requires 24 SRAM bytes"), "{error:?}");
    run(24).unwrap();
}

#[test]
fn dilated_window_transfers_only_accessed_elements() {
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: add", "class: compare");
    let file = dilated_window_timetable();
    let simulation = run_timetable(&platform_source, file);
    let read_bytes = simulation
        .tracker
        .events()
        .iter()
        .filter(|event| event.contains("MemoryAccess: ReadRequest:"))
        .map(|event| {
            event
                .split("MemoryAccess: ReadRequest: ")
                .nth(1)
                .unwrap()
                .split('@')
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap()
        })
        .sum::<usize>();
    assert_eq!(read_bytes, 96);
    let Task::ProgramTask { config } = simulation.timetable.task_by_id(1).unwrap() else {
        panic!("expected program task")
    };
    assert_eq!(config.planned_traffic_bytes().unwrap(), (12, 4));
    assert_eq!(
        simulation
            .platform
            .pe("pe0")
            .unwrap()
            .machine_ops()
            .by_operation["compare"],
        2
    );
    assert_eq!(simulation.platform.pe("pe0").unwrap().peak_sram_bytes(), 24);
}

#[test]
fn max_reduction_compares_broadcast_values_but_reads_singleton_input_once() {
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: add", "class: compare");
    let file = broadcast_max_reduction_timetable();
    let simulation = run_timetable(&platform_source, file);
    assert_eq!(
        simulation
            .platform
            .pe("pe0")
            .unwrap()
            .machine_ops()
            .by_operation["compare"],
        2
    );
    let read_request_count = simulation
        .tracker
        .events()
        .iter()
        .filter(|event| event.contains("MemoryAccess: ReadRequest: 32@1000"))
        .count();
    assert_eq!(read_request_count, 1);
}

#[test]
fn tiled_fused_matrix_epilogue_runs_with_working_set_larger_than_sram() {
    let file = matrix_epilogue_timetable();
    let simulation = run_timetable(
        &matrix_epilogue_platform(MATRIX_EPILOGUE_SRAM_BYTES),
        file.clone(),
    );

    assert_matrix_epilogue_operation_counts(&simulation);
    let events = simulation.tracker.events();
    assert_eq!(memory_request_bytes(&events, "ReadRequest"), 263_680);
    assert_eq!(
        memory_request_bytes(&events, "WriteNonPostedRequest"),
        16_384
    );

    // One tile needs 5192 bytes: A and B slices, bias, floor, output,
    // and two local values. The full logical working set is much larger.
    let too_small = matrix_epilogue_platform(MATRIX_EPILOGUE_TILE_BYTES - 1);
    assert_rejected_before_loading(
        &too_small,
        file,
        &format!("requires {MATRIX_EPILOGUE_TILE_BYTES} SRAM bytes"),
    );
}
