// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn excluded_sum_counts_valid_body_elements_and_accumulations() {
    for chunk in [1, 4] {
        for (body, expected_mul, expected_add) in [
            (
                "{ read: { tensor: A, indices: [{ affine: { terms: { k: 1 }, offset: -1 } }], bounds: exclude } }",
                0,
                1,
            ),
            (
                "{ mul: [{ read: { tensor: A, indices: [{ affine: { terms: { k: 1 }, offset: -1 } }], bounds: exclude } }, { read: { tensor: A, indices: [{ affine: { terms: { k: 1 }, offset: -1 } }], bounds: exclude } }] }",
                2,
                1,
            ),
            (
                "{ mul: [{ read: { tensor: A, indices: [{ affine: { terms: { k: 1 }, offset: -1 } }], bounds: exclude } }, { read: { tensor: B, indices: [{ affine: { terms: { k: 1 }, offset: -2 } }], bounds: exclude } }] }",
                1,
                0,
            ),
            (
                "{ mul: [{ read: { tensor: A, indices: [{ affine: { terms: { k: 1 }, offset: -4 } }], bounds: exclude } }, { read: { tensor: B, indices: [{ affine: { terms: { k: 1 }, offset: -2 } }], bounds: exclude } }] }",
                0,
                0,
            ),
        ] {
            let platform = simulate(&format!(
                r"
nodes:
 - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: fp32, shape: [2] }} }}
 - {{ id: B, kind: tensor, config: {{ addr: 0x3000, dtype: fp32, shape: [2] }} }}
 - {{ id: Y, kind: tensor, config: {{ addr: 0x2000, dtype: fp32, shape: [] }} }}
 - id: sum
   kind: compute
   pe: pe0
   program:
     inputs: [A,B]
     outputs: [Y]
     extents: {{ k: 4 }}
     assignments:
      - output: Y
        axes: []
        expr: {{ reduce: {{ op: sum, over: [k], value: {body} }} }}
   tiling: {{ sizes: {{ k: {chunk} }} }}
edges:
 - {{ from: A, to: sum.0, kind: data }}
 - {{ from: B, to: sum.1, kind: data }}
 - {{ from: sum.0, to: Y, kind: data }}
"
            ));
            let ops = platform.pe("pe0").unwrap().machine_ops();
            assert_eq!(
                ops.by_operation.get("add").copied().unwrap_or(0),
                expected_add,
                "chunk={chunk}"
            );
            assert_eq!(
                ops.by_operation.get("mul").copied().unwrap_or(0),
                expected_mul
            );
        }
    }
}

#[test]
fn matrix_cast_occurs_once_after_all_reduction_chunks() {
    let platform = simulate(
        r"
nodes:
 - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp16, shape: [2,5] } }
 - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp16, shape: [5,3] } }
 - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp16, shape: [2,3] } }
 - id: gemm
   kind: compute
   pe: pe0
   program:
     inputs: [A,B]
     outputs: [Y]
     assignments:
      - output: Y
        axes: [m,n]
        expr: { matrix_mac: { execution: { instruction: tensor, accumulator: fp32 }, over: [k], args: [{ read: { tensor: A, indices: [m,k] } }, { read: { tensor: B, indices: [k,n] } }] } }
   tiling: { sizes: { k: 2 } }
edges:
 - { from: A, to: gemm.0, kind: data }
 - { from: B, to: gemm.1, kind: data }
 - { from: gemm.0, to: Y, kind: data }
",
    );
    let pe = platform.pe("pe0").unwrap();
    assert_eq!(pe.machine_ops().by_operation.get("matrix_mac"), Some(&30));
    assert_eq!(pe.machine_ops().by_operation.get("convert"), Some(&6));
    assert_eq!(pe.memory_traffic_bytes(), (50, 12));
}

#[test]
fn quantized_scale_metadata_is_loaded_even_without_arithmetic_indexing() {
    let platform = simulate(
        r"
nodes:
 - { id: B, kind: tensor, config: { addr: 0x1000, dtype: fp4, shape: [65] } }
 - { id: S, kind: tensor, config: { addr: 0x2000, dtype: fp8, shape: [3] } }
 - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: bf16, shape: [65] } }
 - id: decode
   kind: compute
   pe: pe0
   program:
     inputs: [B,S]
     metadata_inputs: [S]
     outputs: [Y]
     assignments:
      - output: Y
        axes: [k]
        expr: { dequantize: [{ read: { tensor: B, indices: [k] } }] }
   tiling: { sizes: { k: 32 } }
edges:
 - { from: B, to: decode.0, kind: data }
 - { from: S, to: decode.1, kind: data }
 - { from: decode.0, to: Y, kind: data }
",
    );
    let pe = platform.pe("pe0").unwrap();
    // 33 packed weight bytes plus 3 scale bytes for each of 3 output tiles.
    assert_eq!(pe.memory_traffic_bytes(), (42, 130));
    assert_eq!(pe.machine_ops().by_operation.get("dequantize"), Some(&65));
}

#[test]
fn softmax_row_larger_than_sram_runs_with_explicit_recompute_and_reread() {
    let platform = simulate(
        r"
nodes:
 - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1,1025] } }
 - { id: Y, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [1,1025] } }
 - id: softmax
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [Y]
     temporaries: { M: { dtype: fp32 }, S: { dtype: fp32 } }
     assignments:
      - output: M
        axes: [r]
        expr: { reduce: { op: max, over: [k], value: { read: { tensor: A, indices: [r,k] } } } }
      - output: S
        axes: [r]
        expr: { reduce: { op: sum, over: [k], value: { exp: [{ sub: [{ read: { tensor: A, indices: [r,k] } }, { read: { tensor: M, indices: [r] } }] }] } } }
      - output: Y
        axes: [r,k]
        expr: { div: [{ exp: [{ sub: [{ read: { tensor: A, indices: [r,k] } }, { read: { tensor: M, indices: [r] } }] }] }, { read: { tensor: S, indices: [r] } }] }
   tiling: { sizes: { k: 64 } }
edges:
 - { from: A, to: softmax.0, kind: data }
 - { from: softmax.0, to: Y, kind: data }
",
    );
    let pe = platform.pe("pe0").unwrap();
    let ops = pe.machine_ops();
    assert_eq!(pe.memory_traffic_bytes(), (143_500, 4100));
    assert_eq!(ops.by_operation.get("compare"), Some(&17408));
    assert_eq!(ops.by_operation.get("add"), Some(&17408));
    assert_eq!(ops.by_operation.get("exp"), Some(&18450));
    assert_eq!(ops.by_operation.get("sub"), Some(&18450));
    assert_eq!(ops.by_operation.get("div"), Some(&1025));
    assert!(pe.peak_sram_bytes() <= 2304);
}

#[test]
fn program_preflight_rejects_unsupported_mode_and_sram_before_execution() {
    use std::collections::BTreeMap;

    use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
    use gwr_models::processing_element::task::ProgramTaskConfig;
    use gwr_models::processing_element::tensors::dtype::DataType;
    use gwr_models::processing_element::tensors::{Tensor, TensorView};
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        10000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Platform::from_string(
        &engine,
        &clock,
        &PLATFORM.replace("sram_bytes: 2304", "sram_bytes: 63"),
    )
    .unwrap();
    let program:ProgramSpec=yaml_serde::from_str(r"
inputs: [A,B]
outputs: [Y]
assignments:
 - output: Y
   axes: [m,n]
   expr: { matrix_mac: { execution: { instruction: tensor, accumulator: fp32 }, over: [k], args: [{ read: { tensor: A, indices: [m,k] } }, { read: { tensor: B, indices: [k,n] } }] } }
").unwrap();
    let view = |shape: &[usize], addr| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp16, addr).unwrap())
    };
    let mut config = ProgramTaskConfig {
        pipeline: None,
        id: "preflight_matrix".into(),
        program,
        tiling: Some(TileSpec {
            sizes: BTreeMap::from([("k".into(), 2)]),
        }),
        inputs: vec![view(&[2, 5], 4096), view(&[5, 3], 8192)],
        outputs: vec![view(&[2, 3], 12288)],
    };
    let pe = platform.pe("pe0").unwrap();
    assert!(
        pe.preflight_program_task(&config)
            .unwrap_err()
            .0
            .contains("64 SRAM bytes")
    );
    if let gwr_models::processing_element::program::syntax::ExprSpec::MatrixMac {
        execution: Some(mode),
        ..
    } = &mut config.program.assignments[0].expr
    {
        mode.instruction = "unsupported".into();
    }
    assert!(
        pe.preflight_program_task(&config)
            .unwrap_err()
            .0
            .contains("unsupported capability")
    );
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert!(pe.machine_ops().by_operation.is_empty());
}

#[test]
fn reused_slot_waits_for_the_slow_consumer_on_an_independent_branch() {
    let source = r"
nodes:
 - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
 - { id: Y, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [4] } }
 - id: lifetime
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [Y]
     temporaries: { T0: { dtype: fp32 }, T1: { dtype: fp32 }, T2: { dtype: fp32 }, T3: { dtype: fp32 } }
     assignments:
      - output: T0
        axes: [i]
        expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: A, indices: [i] } }] }
      - output: T1
        axes: [i]
        expr: { add: [{ read: { tensor: T0, indices: [i] } }, { read: { tensor: A, indices: [i] } }] }
      - output: T2
        axes: [i]
        expr: { exp: [{ read: { tensor: T0, indices: [i] } }] }
      - output: T3
        axes: [i]
        expr: { add: [{ read: { tensor: T1, indices: [i] } }, { read: { tensor: A, indices: [i] } }] }
      - output: Y
        axes: [i]
        expr: { add: [{ read: { tensor: T3, indices: [i] } }, { read: { tensor: T2, indices: [i] } }] }
edges:
 - { from: A, to: lifetime.0, kind: data }
 - { from: lifetime.0, to: Y, kind: data }
";
    let (platform, events) = simulate_platform_with_events(
        source,
        &PLATFORM.replace(
            "class: unary, ops_per_tick: 8",
            "class: unary, ops_per_tick: 0.01",
        ),
    );
    let slow_finished = events
        .iter()
        .position(|event| event.contains("assignment T2 end"))
        .unwrap();
    let reuse_started = events
        .iter()
        .position(|event| event.contains("assignment T3 begin"))
        .unwrap();
    assert!(
        slow_finished < reuse_started,
        "T3 reuses T0 storage only after both branch consumers finish"
    );
    assert_eq!(
        platform
            .pe("pe0")
            .unwrap()
            .machine_ops()
            .by_operation
            .get("exp"),
        Some(&4)
    );
}

#[test]
fn nested_absmax_body_streams_and_counts_body_work_before_comparisons() {
    let platform = simulate(
        r"
nodes:
 - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1,1025] } }
 - { id: Neg, kind: tensor, config: { addr: 0x6000, dtype: fp32, shape: [] } }
 - { id: Y, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [1] } }
 - id: absmax
   kind: compute
   pe: pe0
   program:
     inputs: [A,Neg]
     outputs: [Y]
     assignments:
      - output: Y
        axes: [r]
        expr: { reduce: { op: max, over: [k], value: { maximum: [{ read: { tensor: A, indices: [r,k] } }, { mul: [{ read: { tensor: A, indices: [r,k] } }, { read: { tensor: Neg, indices: [] } }] }] } } }
   tiling: { sizes: { k: 128 } }
edges:
 - { from: A, to: absmax.0, kind: data }
 - { from: Neg, to: absmax.1, kind: data }
 - { from: absmax.0, to: Y, kind: data }
",
    );
    let pe = platform.pe("pe0").unwrap();
    assert_eq!(pe.memory_traffic_bytes(), (4136, 4));
    assert_eq!(pe.machine_ops().by_operation.get("maximum"), Some(&1025));
    assert_eq!(pe.machine_ops().by_operation.get("mul"), Some(&1025));
    assert_eq!(pe.machine_ops().by_operation.get("compare"), Some(&1024));
    assert!(pe.peak_sram_bytes() <= 2304);
}

#[test]
fn nested_absmax_excludes_tail_elements_from_arithmetic_and_comparisons() {
    let platform = simulate(
        r"
nodes:
 - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1025] } }
 - { id: Neg, kind: tensor, config: { addr: 0x6000, dtype: fp32, shape: [] } }
 - { id: Y, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [9] } }
 - id: absmax_tail
   kind: compute
   pe: pe0
   program:
     inputs: [A,Neg]
     outputs: [Y]
     extents: { b: 9, k: 128 }
     assignments:
      - output: Y
        axes: [b]
        expr: { reduce: { op: max, over: [k], value: { maximum: [{ read: { tensor: A, indices: [{ affine: { terms: { b: 128, k: 1 }, offset: 0 } }], bounds: exclude } }, { mul: [{ read: { tensor: A, indices: [{ affine: { terms: { b: 128, k: 1 }, offset: 0 } }], bounds: exclude } }, { read: { tensor: Neg, indices: [] } }] }] } } }
   tiling: { sizes: { b: 1, k: 64 } }
edges:
 - { from: A, to: absmax_tail.0, kind: data }
 - { from: Neg, to: absmax_tail.1, kind: data }
 - { from: absmax_tail.0, to: Y, kind: data }
",
    );
    let pe = platform.pe("pe0").unwrap();
    assert_eq!(pe.memory_traffic_bytes(), (4172, 36));
    assert_eq!(pe.machine_ops().by_operation.get("maximum"), Some(&1025));
    assert_eq!(pe.machine_ops().by_operation.get("mul"), Some(&1025));
    assert_eq!(pe.machine_ops().by_operation.get("compare"), Some(&1016));
}

fn simulate(source: &str) -> Rc<Platform> {
    simulate_with_events(source).0
}

fn simulate_with_events(source: &str) -> (Rc<Platform>, Vec<String>) {
    simulate_platform_with_events(source, PLATFORM)
}

const PLATFORM: &str = r"
memory_maps:
 - name: mm
   devices: [mem]
operation_classes:
 matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
 unary: { operations: [convert, dequantize, exp], arity: 1 }
 binary: { operations: [add, mul, sub, div, compare, maximum], arity: 2 }
processing_elements:
 - name: pe0
   memory_map: mm
   config:
     sram_bytes: 2304
     compute:
       engines:
         matrix:
           capabilities: [{ class: matrix, input_dtypes: [[fp16, fp16]], ops_per_tick: 8, execution: { instruction: tensor, accumulator: fp32 } }]
         vector:
           capabilities: [{ class: unary, ops_per_tick: 8 }, { class: binary, ops_per_tick: 8 }]
memories:
 - name: mem
   kind: hbm
   base_address: 0
   config: { capacity_bytes: 1MiB }
connections:
 - connect: [pe.pe0, mem.mem]
";

fn simulate_platform_with_events(
    source: &str,
    platform_source: &str,
) -> (Rc<Platform>, Vec<String>) {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        10000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, platform_source).unwrap());
    let file = TimetableFile::from_string(source).unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    (platform, tracker.events())
}
