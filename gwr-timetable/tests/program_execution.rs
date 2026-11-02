// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::SRAM_SLOT_ALIGNMENT_BYTES;
use gwr_models::processing_element::task::ProgramTaskConfig;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::Tracker;
use gwr_track::log::Level;
use gwr_track::test_helpers::TestTracker;
use yaml_serde::from_str;

// Fixture dimensions and storage sizes are shared by the timetables and
// assertions.
const FP16_BYTES: usize = 2;
const FP32_BYTES: usize = 4;
const SRAM_BYTES: usize = 2304;
const TRACKER_INITIAL_ID: u64 = 10_000;
const MATRIX_ROWS: usize = 2;
const MATRIX_COLUMNS: usize = 3;
const MATRIX_REDUCTION_ELEMENTS: usize = 5;
const MATRIX_REDUCTION_CHUNK: usize = 2;
const SOFTMAX_ELEMENTS: usize = 1025;
const SOFTMAX_ROWS: usize = 3;
const LOCAL_ROWS: usize = 3;
const LOCAL_ELEMENTS: usize = 129;
const LOCAL_CHUNK: usize = 16;
const LOCAL_CHUNK_SRAM_BYTES: usize = LOCAL_CHUNK * FP32_BYTES * 3;
const LOCAL_ROW_SRAM_BYTES: usize = (LOCAL_ELEMENTS * FP32_BYTES)
    .next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES)
    + (LOCAL_CHUNK * FP32_BYTES).next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);
const SOFTMAX_CHUNK: usize = 64;
const ABSMAX_ELEMENTS: usize = 1025;
const ABSMAX_CHUNK: usize = 128;
const ABSMAX_BLOCK_ELEMENTS: usize = 128;
const ABSMAX_TAIL_CHUNK: usize = 64;
const ABSMAX_BLOCKS: usize = ABSMAX_ELEMENTS.div_ceil(ABSMAX_BLOCK_ELEMENTS);
const BRANCH_ELEMENTS: usize = 4;
const EXCLUDED_SUM_ELEMENTS: usize = 2;
const EXCLUDED_SUM_EXTENT: usize = 4;
const FIRST_READ_OFFSET: usize = 1;
const SECOND_READ_OFFSET: usize = 2;
const OUTSIDE_READ_OFFSET: usize = EXCLUDED_SUM_EXTENT;
const COMPUTE_OPS_PER_TICK: usize = 8;
const SLOW_UNARY_OPS_PER_TICK: f64 = 0.01;

fn assert_schedule_sram_rejected_without_execution(source: &str, sram_bytes: usize) {
    let tracker = Rc::new(TestTracker::new(TRACKER_INITIAL_ID, Level::Trace));
    let tracking: Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(
        Platform::from_string(&engine, &clock, &compute_platform_yaml(sram_bytes)).unwrap(),
    );
    let file = TimetableFile::from_string(source).unwrap();
    let result = Timetable::new(engine.top(), file.into_graph().unwrap(), &platform);
    let error = match result {
        Ok(_) => panic!("automatic schedule must exceed {sram_bytes} bytes of SRAM"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("SRAM"), "{error}");
    let pe = platform.pe("pe0").unwrap();
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert!(pe.machine_ops().by_operation.is_empty());
}

fn validation_platform(sram_bytes: usize) -> Platform {
    let tracker = Rc::new(TestTracker::new(TRACKER_INITIAL_ID, Level::Trace));
    let tracking: Tracker = tracker;
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    Platform::from_string(&engine, &clock, &compute_platform_yaml(sram_bytes)).unwrap()
}

fn simulate_timetable(source: &str) -> Rc<Platform> {
    simulate_timetable_with_events(source).0
}

fn simulate_timetable_with_events(source: &str) -> (Rc<Platform>, Vec<String>) {
    simulate_timetable_on_platform_with_events(source, &compute_platform_yaml(SRAM_BYTES))
}

fn compute_platform_yaml(sram_bytes: usize) -> String {
    format!(
        r"
memory_maps:
 - name: mm
   devices: [mem]
operation_classes:
 matrix: {{ operations: [matrix_mac], arity: 2, flops_per_op: 2 }}
 unary: {{ operations: [convert, exp], arity: 1 }}
 binary: {{ operations: [add, mul, sub, div, compare, maximum], arity: 2 }}
processing_elements:
 - name: pe0
   memory_map: mm
   config:
     sram_bytes: {sram_bytes}
     compute:
       execution_units:
         matrix:
           capabilities:
             - class: matrix
               input_dtypes: [[fp16, fp16]]
               ops_per_tick: {COMPUTE_OPS_PER_TICK}
               accumulator: fp32
         vector:
           capabilities:
             - {{ class: unary, ops_per_tick: {COMPUTE_OPS_PER_TICK} }}
             - {{ class: binary, ops_per_tick: {COMPUTE_OPS_PER_TICK} }}
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

fn simulate_timetable_on_platform_with_events(
    source: &str,
    platform_source: &str,
) -> (Rc<Platform>, Vec<String>) {
    let tracker = Rc::new(TestTracker::new(TRACKER_INITIAL_ID, Level::Trace));
    let tracking: Tracker = tracker.clone();
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
    let planned = timetable.workload_totals().unwrap();
    assert_eq!(
        platform.pe("pe0").unwrap().memory_traffic_bytes(),
        (planned.read_bytes, planned.written_bytes),
        "planned boundary traffic must match completed execution",
    );
    (platform, tracker.events())
}

fn matrix_cast_timetable() -> String {
    format!(
        r"
nodes:
 - id: A
   kind: tensor
   config:
     addr: 0x1000
     dtype: fp16
     shape: [{MATRIX_ROWS},{MATRIX_REDUCTION_ELEMENTS}]
 - id: B
   kind: tensor
   config:
     addr: 0x2000
     dtype: fp16
     shape: [{MATRIX_REDUCTION_ELEMENTS},{MATRIX_COLUMNS}]
 - id: Y
   kind: tensor
   config:
     addr: 0x3000
     dtype: fp16
     shape: [{MATRIX_ROWS},{MATRIX_COLUMNS}]
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
              - read: {{ tensor: A, indices: [m,k] }}
              - read: {{ tensor: B, indices: [k,n] }}
   tiling: {{ sizes: {{ k: {MATRIX_REDUCTION_CHUNK} }} }}
edges:
 - {{ from: A, to: gemm.0, kind: data }}
 - {{ from: B, to: gemm.1, kind: data }}
 - {{ from: gemm.0, to: Y, kind: data }}
"
    )
}

fn softmax_timetable(rows: usize) -> String {
    format!(
        r"
nodes:
 - id: A
   kind: tensor
   config:
     addr: 0x1000
     dtype: fp32
     shape: [{rows},{SOFTMAX_ELEMENTS}]
 - id: Y
   kind: tensor
   config:
     addr: 0x10000
     dtype: fp32
     shape: [{rows},{SOFTMAX_ELEMENTS}]
 - id: softmax
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [Y]
     temporaries: {{ M: {{ dtype: fp32 }}, S: {{ dtype: fp32 }} }}
     assignments:
      - output: M
        axes: [r]
        expr:
          reduce:
            op: max
            over: [k]
            value:
              read: {{ tensor: A, indices: [r,k] }}
      - output: S
        axes: [r]
        expr:
          reduce:
            op: sum
            over: [k]
            value:
              exp:
                - sub:
                    - read: {{ tensor: A, indices: [r,k] }}
                    - read: {{ tensor: M, indices: [r] }}
      - output: Y
        axes: [r,k]
        expr:
          div:
            - exp:
                - sub:
                    - read: {{ tensor: A, indices: [r,k] }}
                    - read: {{ tensor: M, indices: [r] }}
            - read: {{ tensor: S, indices: [r] }}
   tiling: {{ sizes: {{ r: 1, k: {SOFTMAX_CHUNK} }} }}
edges:
 - {{ from: A, to: softmax.0, kind: data }}
 - {{ from: softmax.0, to: Y, kind: data }}
"
    )
}

fn independent_statistics_timetable() -> String {
    format!(
        r"
nodes:
 - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: fp32, shape: [{LOCAL_ROWS},{LOCAL_ELEMENTS}] }} }}
 - {{ id: Y, kind: tensor, config: {{ addr: 0x4000, dtype: fp32, shape: [{LOCAL_ROWS},{LOCAL_ELEMENTS}] }} }}
 - id: statistics
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [Y]
     temporaries: {{ M: {{ dtype: fp32 }}, S: {{ dtype: fp32 }} }}
     assignments:
      - output: M
        axes: [r]
        expr:
          reduce:
            op: max
            over: [k]
            value:
              read: {{ tensor: A, indices: [r, k] }}
      - output: S
        axes: [r]
        expr:
          reduce:
            op: sum
            over: [k]
            value:
              read: {{ tensor: A, indices: [r, k] }}
      - output: Y
        axes: [r,k]
        expr:
          add:
            - read: {{ tensor: M, indices: [r] }}
            - read: {{ tensor: S, indices: [r] }}
   tiling: {{ sizes: {{ r: 1, k: {LOCAL_CHUNK} }} }}
edges:
 - {{ from: A, to: statistics.0, kind: data }}
 - {{ from: statistics.0, to: Y, kind: data }}
"
    )
}

fn local_temporary_timetable(pipeline: bool) -> String {
    let output_shape = if pipeline {
        format!("[{LOCAL_ROWS},{LOCAL_ELEMENTS}]")
    } else {
        format!("[{LOCAL_ROWS}]")
    };
    let output_axes = if pipeline { "[r,k]" } else { "[r]" };
    let output_expr = if pipeline {
        r"          add:
            - read: { tensor: T, indices: [r,k] }
            - read: { tensor: T, indices: [r,k] }"
            .to_string()
    } else {
        r"          reduce:
            op: sum
            over: [k]
            value:
              read: { tensor: T, indices: [r,k] }"
            .to_string()
    };
    format!(
        r"
nodes:
 - id: A
   kind: tensor
   config:
     addr: 0x1000
     dtype: fp32
     shape: [{LOCAL_ROWS},{LOCAL_ELEMENTS}]
 - id: Y
   kind: tensor
   config:
     addr: 0x4000
     dtype: fp32
     shape: {output_shape}
 - id: local
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [Y]
     temporaries: {{ T: {{ dtype: fp32 }} }}
     assignments:
      - output: T
        axes: [r,k]
        expr:
          exp:
            - read: {{ tensor: A, indices: [r,k] }}
      - output: Y
        axes: {output_axes}
        expr:
{output_expr}
   tiling: {{ sizes: {{ r: 1, k: {LOCAL_CHUNK} }} }}
edges:
 - {{ from: A, to: local.0, kind: data }}
 - {{ from: local.0, to: Y, kind: data }}
"
    )
}

// Reference implementation with three graph nodes and external scalar
// statistics. The fused version retains these statistics locally instead.
fn three_pass_softmax_timetable() -> String {
    format!(
        r"
nodes:
 - id: A
   kind: tensor
   config:
     addr: 0x1000
     dtype: fp32
     shape: [1,{SOFTMAX_ELEMENTS}]
 - id: M
   kind: tensor
   config:
     addr: 0x6000
     dtype: fp32
     shape: [1]
 - id: S
   kind: tensor
   config:
     addr: 0x7000
     dtype: fp32
     shape: [1]
 - id: Y
   kind: tensor
   config:
     addr: 0x4000
     dtype: fp32
     shape: [1,{SOFTMAX_ELEMENTS}]
 - id: row_max
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [M]
     assignments:
      - output: M
        axes: [r]
        expr:
          reduce:
            op: max
            over: [k]
            value:
              read: {{ tensor: A, indices: [r,k] }}
   tiling: {{ sizes: {{ k: {SOFTMAX_CHUNK} }} }}
 - id: row_sum
   kind: compute
   pe: pe0
   program:
     inputs: [A,M]
     outputs: [S]
     assignments:
      - output: S
        axes: [r]
        expr:
          reduce:
            op: sum
            over: [k]
            value:
              exp:
                - sub:
                    - read: {{ tensor: A, indices: [r,k] }}
                    - read: {{ tensor: M, indices: [r] }}
   tiling: {{ sizes: {{ k: {SOFTMAX_CHUNK} }} }}
 - id: normalize
   kind: compute
   pe: pe0
   program:
     inputs: [A,M,S]
     outputs: [Y]
     assignments:
      - output: Y
        axes: [r,k]
        expr:
          div:
            - exp:
                - sub:
                    - read: {{ tensor: A, indices: [r,k] }}
                    - read: {{ tensor: M, indices: [r] }}
            - read: {{ tensor: S, indices: [r] }}
   tiling: {{ sizes: {{ k: {SOFTMAX_CHUNK} }} }}
edges:
 - {{ from: A, to: row_max.0, kind: data }}
 - {{ from: row_max.0, to: M, kind: data }}
 - {{ from: A, to: row_sum.0, kind: data }}
 - {{ from: M, to: row_sum.1, kind: data }}
 - {{ from: row_sum.0, to: S, kind: data }}
 - {{ from: A, to: normalize.0, kind: data }}
 - {{ from: M, to: normalize.1, kind: data }}
 - {{ from: S, to: normalize.2, kind: data }}
 - {{ from: normalize.0, to: Y, kind: data }}
"
    )
}

fn absmax_timetable() -> String {
    format!(
        r"
nodes:
 - id: A
   kind: tensor
   config:
     addr: 0x1000
     dtype: fp32
     shape: [1,{ABSMAX_ELEMENTS}]
 - id: Neg
   kind: tensor
   config:
     addr: 0x6000
     dtype: fp32
     shape: []
 - id: Y
   kind: tensor
   config:
     addr: 0x4000
     dtype: fp32
     shape: [1]
 - id: absmax
   kind: compute
   pe: pe0
   program:
     inputs: [A,Neg]
     outputs: [Y]
     assignments:
      - output: Y
        axes: [r]
        expr:
          reduce:
            op: max
            over: [k]
            value:
              maximum:
                - read: {{ tensor: A, indices: [r,k] }}
                - mul:
                    - read: {{ tensor: A, indices: [r,k] }}
                    - read: {{ tensor: Neg, indices: [] }}
   tiling: {{ sizes: {{ k: {ABSMAX_CHUNK} }} }}
edges:
 - {{ from: A, to: absmax.0, kind: data }}
 - {{ from: Neg, to: absmax.1, kind: data }}
 - {{ from: absmax.0, to: Y, kind: data }}
"
    )
}

fn absmax_tail_timetable() -> String {
    format!(
        r"
nodes:
 - id: A
   kind: tensor
   config:
     addr: 0x1000
     dtype: fp32
     shape: [{ABSMAX_ELEMENTS}]
 - id: Neg
   kind: tensor
   config:
     addr: 0x6000
     dtype: fp32
     shape: []
 - id: Y
   kind: tensor
   config:
     addr: 0x4000
     dtype: fp32
     shape: [{ABSMAX_BLOCKS}]
 - id: absmax_tail
   kind: compute
   pe: pe0
   program:
     inputs: [A,Neg]
     outputs: [Y]
     extents: {{ b: {ABSMAX_BLOCKS}, k: {ABSMAX_BLOCK_ELEMENTS} }}
     assignments:
      - output: Y
        axes: [b]
        expr:
          reduce:
            op: max
            over: [k]
            value:
              maximum:
                - read:
                    tensor: A
                    indices:
                      - affine:
                          terms: {{ b: {ABSMAX_BLOCK_ELEMENTS}, k: 1 }}
                          offset: 0
                    bounds: exclude
                - mul:
                    - read:
                        tensor: A
                        indices:
                          - affine:
                              terms: {{ b: {ABSMAX_BLOCK_ELEMENTS}, k: 1 }}
                              offset: 0
                        bounds: exclude
                    - read: {{ tensor: Neg, indices: [] }}
   tiling: {{ sizes: {{ b: 1, k: {ABSMAX_TAIL_CHUNK} }} }}
edges:
 - {{ from: A, to: absmax_tail.0, kind: data }}
 - {{ from: Neg, to: absmax_tail.1, kind: data }}
 - {{ from: absmax_tail.0, to: Y, kind: data }}
"
    )
}

fn excluded_sum_timetable(body: &str, chunk: usize) -> String {
    let body = body.trim().replace('\n', "\n              ");
    format!(
        r"
nodes:
 - id: A
   kind: tensor
   config:
     addr: 0x1000
     dtype: fp32
     shape: [{EXCLUDED_SUM_ELEMENTS}]
 - id: B
   kind: tensor
   config:
     addr: 0x3000
     dtype: fp32
     shape: [{EXCLUDED_SUM_ELEMENTS}]
 - id: Y
   kind: tensor
   config:
     addr: 0x2000
     dtype: fp32
     shape: []
 - id: sum
   kind: compute
   pe: pe0
   program:
     inputs: [A,B]
     outputs: [Y]
     extents: {{ k: {EXCLUDED_SUM_EXTENT} }}
     assignments:
      - output: Y
        axes: []
        expr:
          reduce:
            op: sum
            over: [k]
            value:
              {body}
   tiling: {{ sizes: {{ k: {chunk} }} }}
edges:
 - {{ from: A, to: sum.0, kind: data }}
 - {{ from: B, to: sum.1, kind: data }}
 - {{ from: sum.0, to: Y, kind: data }}
"
    )
}

fn matrix_validation_config() -> ProgramTaskConfig {
    use std::collections::BTreeMap;

    use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
    use gwr_models::processing_element::tensors::dtype::DataType;
    use gwr_models::processing_element::tensors::{Tensor, TensorView};

    let program: ProgramSpec = from_str(
        r"
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
         - read: { tensor: A, indices: [m,k] }
         - read: { tensor: B, indices: [k,n] }
",
    )
    .unwrap();
    let view = |shape: &[usize], addr| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp16, addr).unwrap())
    };
    ProgramTaskConfig {
        id: "matrix_validation".into(),
        program,
        tiling: Some(TileSpec {
            sizes: BTreeMap::from([("k".into(), MATRIX_REDUCTION_CHUNK)]),
        }),
        inputs: vec![
            view(&[MATRIX_ROWS, MATRIX_REDUCTION_ELEMENTS], 0x1000),
            view(&[MATRIX_REDUCTION_ELEMENTS, MATRIX_COLUMNS], 0x2000),
        ],
        outputs: vec![view(&[MATRIX_ROWS, MATRIX_COLUMNS], 0x3000)],
    }
}

fn branched_lifetime_timetable() -> String {
    format!(
        r"
nodes:
 - id: A
   kind: tensor
   config:
     addr: 0x1000
     dtype: fp32
     shape: [{BRANCH_ELEMENTS}]
 - id: Y
   kind: tensor
   config:
     addr: 0x4000
     dtype: fp32
     shape: [{BRANCH_ELEMENTS}]
 - id: lifetime
   kind: compute
   pe: pe0
   program:
     inputs: [A]
     outputs: [Y]
     temporaries:
       T0: {{ dtype: fp32 }}
       T1: {{ dtype: fp32 }}
       T2: {{ dtype: fp32 }}
       T3: {{ dtype: fp32 }}
     assignments:
      - output: T0
        axes: [i]
        expr:
          add:
            - read: {{ tensor: A, indices: [i] }}
            - read: {{ tensor: A, indices: [i] }}
      - output: T1
        axes: [i]
        expr:
          add:
            - read: {{ tensor: T0, indices: [i] }}
            - read: {{ tensor: A, indices: [i] }}
      - output: T2
        axes: [i]
        expr:
          exp:
            - read: {{ tensor: T0, indices: [i] }}
      - output: T3
        axes: [i]
        expr:
          add:
            - read: {{ tensor: T1, indices: [i] }}
            - read: {{ tensor: A, indices: [i] }}
      - output: Y
        axes: [i]
        expr:
          add:
            - read: {{ tensor: T3, indices: [i] }}
            - read: {{ tensor: T2, indices: [i] }}
edges:
 - {{ from: A, to: lifetime.0, kind: data }}
 - {{ from: lifetime.0, to: Y, kind: data }}
"
    )
}

#[test]
fn excluded_sum_counts_valid_body_elements_and_accumulations() {
    let read = |tensor: &str, offset: usize| {
        format!(
            r"
read:
  tensor: {tensor}
  indices:
    - affine:
        terms: {{ k: 1 }}
        offset: -{offset}
  bounds: exclude
"
        )
    };
    let multiply = |left: &str, right: &str| {
        let left = left.trim().replace('\n', "\n    ");
        let right = right.trim().replace('\n', "\n    ");
        format!(
            r"mul:
  - {left}
  - {right}"
        )
    };
    let a = read("A", FIRST_READ_OFFSET);
    let b = read("B", SECOND_READ_OFFSET);
    let outside_a = read("A", OUTSIDE_READ_OFFSET);
    // A read is valid for offset <= k < offset + tensor length, clipped to k's
    // extent.
    let valid_end = EXCLUDED_SUM_EXTENT.min(FIRST_READ_OFFSET + EXCLUDED_SUM_ELEMENTS);
    let valid_a = valid_end - FIRST_READ_OFFSET;
    let valid_pair = valid_end.saturating_sub(SECOND_READ_OFFSET);
    let valid_outside = EXCLUDED_SUM_EXTENT.saturating_sub(OUTSIDE_READ_OFFSET);
    for chunk in [1, EXCLUDED_SUM_EXTENT] {
        for (body, valid_elements, multiplies_per_element) in [
            (a.clone(), valid_a, 0),
            (multiply(&a, &a), valid_a, 1),
            (multiply(&a, &b), valid_pair, 1),
            (multiply(&outside_a, &b), valid_outside, 1),
        ] {
            let expected_mul = valid_elements * multiplies_per_element;
            // Summing zero or one valid element needs no addition.
            let expected_add = valid_elements.saturating_sub(1);
            let platform = simulate_timetable(&excluded_sum_timetable(&body, chunk));
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
fn chunked_matrix_reduction_counts_one_output_conversion_per_element() {
    let platform = simulate_timetable(&matrix_cast_timetable());

    let pe = platform.pe("pe0").unwrap();
    let output_elements = MATRIX_ROWS * MATRIX_COLUMNS;
    let input_elements =
        MATRIX_ROWS * MATRIX_REDUCTION_ELEMENTS + MATRIX_REDUCTION_ELEMENTS * MATRIX_COLUMNS;
    let matrix_macs = output_elements * MATRIX_REDUCTION_ELEMENTS;
    // Cast each output once, after accumulating every reduction chunk in fp32.
    assert_eq!(
        pe.machine_ops().by_operation.get("matrix_mac"),
        Some(&matrix_macs)
    );
    assert_eq!(
        pe.machine_ops().by_operation.get("convert"),
        Some(&output_elements)
    );
    assert_eq!(
        pe.memory_traffic_bytes(),
        (input_elements * FP16_BYTES, output_elements * FP16_BYTES)
    );
}

#[test]
fn three_node_softmax_computes_each_row_statistic_once() {
    let platform = simulate_timetable(&three_pass_softmax_timetable());

    let pe = platform.pe("pe0").unwrap();
    let ops = pe.machine_ops();
    let row_bytes = SOFTMAX_ELEMENTS * FP32_BYTES;
    let chunks = SOFTMAX_ELEMENTS.div_ceil(SOFTMAX_CHUNK);
    let reduction_ops = SOFTMAX_ELEMENTS - 1;
    // Sub/exp run once during the sum pass and once during normalization.
    let sub_exp_ops = SOFTMAX_ELEMENTS * 2;
    assert_eq!(ops.by_operation.get("compare"), Some(&reduction_ops));
    assert_eq!(ops.by_operation.get("add"), Some(&reduction_ops));
    assert_eq!(ops.by_operation.get("sub"), Some(&sub_exp_ops));
    assert_eq!(ops.by_operation.get("exp"), Some(&sub_exp_ops));
    assert_eq!(ops.by_operation.get("div"), Some(&SOFTMAX_ELEMENTS));

    // Three full A passes: maximum, sum, normalization. The sum pass also
    // reads M per chunk; normalization reads both M and S per output chunk.
    let row_read_bytes = row_bytes * 3;
    let statistic_read_bytes = chunks * (1 + 2) * FP32_BYTES;
    // Only the scalar statistics and the final row are written, with no
    // full-row exponential intermediate spilled to memory.
    let statistic_write_bytes = 2 * FP32_BYTES;
    assert_eq!(
        pe.memory_traffic_bytes(),
        (
            row_read_bytes + statistic_read_bytes,
            row_bytes + statistic_write_bytes
        )
    );
    assert!(row_bytes > SRAM_BYTES);
    assert!(pe.peak_sram_bytes() <= SRAM_BYTES);
}

#[test]
fn softmax_computes_statistics_once_and_keeps_them_local() {
    for rows in [1, SOFTMAX_ROWS] {
        let platform = simulate_timetable(&softmax_timetable(rows));
        let pe = platform.pe("pe0").unwrap();
        let ops = pe.machine_ops();
        let reduction_ops = rows * (SOFTMAX_ELEMENTS - 1);
        let elements = rows * SOFTMAX_ELEMENTS;
        let sub_exp_ops = elements * 2;
        let row_bytes = SOFTMAX_ELEMENTS * FP32_BYTES;
        assert_eq!(ops.by_operation.get("compare"), Some(&reduction_ops));
        assert_eq!(ops.by_operation.get("add"), Some(&reduction_ops));
        assert_eq!(ops.by_operation.get("sub"), Some(&sub_exp_ops));
        assert_eq!(ops.by_operation.get("exp"), Some(&sub_exp_ops));
        assert_eq!(ops.by_operation.get("div"), Some(&elements));
        assert_eq!(
            pe.memory_traffic_bytes(),
            (rows * row_bytes * 3, rows * row_bytes)
        );
        assert!(row_bytes > SRAM_BYTES);
        assert!(pe.peak_sram_bytes() <= SRAM_BYTES);
    }
}

#[test]
fn inferred_row_statistics_group_overlaps_independent_execution_units() {
    let platform_source = compute_platform_yaml(SRAM_BYTES)
        .replace("[add, mul, sub, div, compare, maximum]", "[mul, sub, div, maximum]")
        .replace("processing_elements:", " sum: { operations: [add], arity: 2 }\n comparison: { operations: [compare], arity: 2 }\nprocessing_elements:")
        .replace("         matrix:", &format!("         sums:\n           capabilities: [{{ class: sum, ops_per_tick: {COMPUTE_OPS_PER_TICK} }}]\n         comparisons:\n           capabilities: [{{ class: comparison, ops_per_tick: {COMPUTE_OPS_PER_TICK} }}]\n         matrix:"));
    let (platform, events) = simulate_timetable_on_platform_with_events(
        &independent_statistics_timetable(),
        &platform_source,
    );
    let event_position = |label: &str| {
        events
            .iter()
            .position(|event| event.contains(label))
            .unwrap()
    };
    let max_started = event_position("assignment M begin");
    let sum_started = event_position("assignment S begin");
    let first_finished = event_position("assignment M end").min(event_position("assignment S end"));
    assert!(max_started < first_finished);
    assert!(sum_started < first_finished);
    let pe = platform.pe("pe0").unwrap();
    let elements = LOCAL_ROWS * LOCAL_ELEMENTS;
    assert_eq!(
        pe.memory_traffic_bytes(),
        (elements * FP32_BYTES * 2, elements * FP32_BYTES)
    );
}

#[test]
fn automatic_schedule_reserves_complete_local_rows_for_downstream_reductions() {
    let platform = simulate_timetable(&local_temporary_timetable(false));
    let pe = platform.pe("pe0").unwrap();
    let elements = LOCAL_ROWS * LOCAL_ELEMENTS;
    let additions = LOCAL_ROWS * (LOCAL_ELEMENTS - 1);
    assert_eq!(pe.machine_ops().by_operation.get("exp"), Some(&elements));
    assert_eq!(pe.machine_ops().by_operation.get("add"), Some(&additions));
    assert_eq!(
        pe.memory_traffic_bytes(),
        (elements * FP32_BYTES, LOCAL_ROWS * FP32_BYTES)
    );
    // T spans a complete row even though its producer streams in chunks.
    assert_eq!(pe.peak_sram_bytes(), LOCAL_ROW_SRAM_BYTES);
    assert!(pe.peak_sram_bytes() <= SRAM_BYTES);
}

#[test]
fn automatic_schedule_reuses_chunk_local_storage_between_pipeline_iterations() {
    let (platform, _) = simulate_timetable_on_platform_with_events(
        &local_temporary_timetable(true),
        &compute_platform_yaml(LOCAL_CHUNK_SRAM_BYTES),
    );
    let pe = platform.pe("pe0").unwrap();
    let elements = LOCAL_ROWS * LOCAL_ELEMENTS;
    assert_eq!(pe.machine_ops().by_operation.get("exp"), Some(&elements));
    assert_eq!(pe.machine_ops().by_operation.get("add"), Some(&elements));
    assert_eq!(
        pe.memory_traffic_bytes(),
        (elements * FP32_BYTES, elements * FP32_BYTES)
    );
    // A full temporary row does not fit this bound, but one pipelined chunk
    // does.
    assert!(pe.peak_sram_bytes() <= LOCAL_CHUNK_SRAM_BYTES);
    assert!(pe.peak_sram_bytes() < LOCAL_ELEMENTS * FP32_BYTES);
}

#[test]
fn automatic_schedule_retains_a_consumed_boundary_output_in_local_storage() {
    let source = local_temporary_timetable(false)
        .replace(
            " - id: local",
            &format!(
                r" - id: T
   kind: tensor
   config:
     addr: 0x8000
     dtype: fp32
     shape: [{LOCAL_ROWS},{LOCAL_ELEMENTS}]
 - id: local"
            ),
        )
        .replace("outputs: [Y]", "outputs: [T,Y]")
        .replace("     temporaries: { T: { dtype: fp32 } }\n", "")
        .replace("from: local.0, to: Y", "from: local.1, to: Y")
        + " - { from: local.0, to: T, kind: data }\n";
    let platform = simulate_timetable(&source);
    let pe = platform.pe("pe0").unwrap();
    let elements = LOCAL_ROWS * LOCAL_ELEMENTS;
    let additions = LOCAL_ROWS * (LOCAL_ELEMENTS - 1);
    assert_eq!(pe.machine_ops().by_operation.get("exp"), Some(&elements));
    assert_eq!(pe.machine_ops().by_operation.get("add"), Some(&additions));
    // T is written to the boundary once, then consumed from its retained row.
    assert_eq!(
        pe.memory_traffic_bytes(),
        (elements * FP32_BYTES, (elements + LOCAL_ROWS) * FP32_BYTES)
    );
    assert_eq!(pe.peak_sram_bytes(), LOCAL_ROW_SRAM_BYTES);
    assert!(pe.peak_sram_bytes() <= SRAM_BYTES);
}

#[test]
fn softmax_sram_rejection_precedes_memory_requests() {
    assert_schedule_sram_rejected_without_execution(&softmax_timetable(1), 1);
}

#[test]
fn materialized_row_sram_rejection_precedes_memory_requests() {
    assert_schedule_sram_rejected_without_execution(
        &local_temporary_timetable(false),
        LOCAL_CHUNK_SRAM_BYTES,
    );
}

#[test]
fn program_validation_rejects_insufficient_sram_before_execution() {
    // Inputs, output and persistent fp32 accumulator each reserve an aligned
    // slot.
    let aligned = |bytes: usize| bytes.next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);
    let output_elements = MATRIX_ROWS * MATRIX_COLUMNS;
    let required_sram = aligned(MATRIX_ROWS * MATRIX_REDUCTION_CHUNK * FP16_BYTES)
        + aligned(MATRIX_REDUCTION_CHUNK * MATRIX_COLUMNS * FP16_BYTES)
        + aligned(output_elements * FP16_BYTES)
        + aligned(output_elements * FP32_BYTES);
    let platform = validation_platform(required_sram - 1);
    let config = matrix_validation_config();
    let pe = platform.pe("pe0").unwrap();
    let error = pe.validate_program_task(&config).unwrap_err();
    assert!(
        error.0.contains(&format!("{required_sram} SRAM bytes")),
        "{error}"
    );
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert!(pe.machine_ops().by_operation.is_empty());
}

#[test]
fn program_validation_rejects_unsupported_accumulator_before_execution() {
    use gwr_models::processing_element::program::syntax::ExprSpec;
    use gwr_models::processing_element::tensors::dtype::DataType;

    // Provide enough SRAM so only the unsupported accumulator can reject this
    // task.
    let platform = validation_platform(SRAM_BYTES);
    let mut config = matrix_validation_config();
    let ExprSpec::MatrixMac { accumulator, .. } = &mut config.program.assignments[0].expr else {
        panic!("matrix fixture must contain a matrix_mac assignment");
    };
    *accumulator = DataType::Int32;
    let pe = platform.pe("pe0").unwrap();
    let error = pe.validate_program_task(&config).unwrap_err();
    assert!(error.0.contains("unsupported capability"), "{error}");
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert!(pe.machine_ops().by_operation.is_empty());
}

#[test]
fn reused_slot_waits_for_the_slow_consumer_on_an_independent_branch() {
    let (platform, events) = simulate_timetable_on_platform_with_events(
        &branched_lifetime_timetable(),
        &compute_platform_yaml(SRAM_BYTES).replace(
            &format!("class: unary, ops_per_tick: {COMPUTE_OPS_PER_TICK}"),
            &format!("class: unary, ops_per_tick: {SLOW_UNARY_OPS_PER_TICK}"),
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
        Some(&BRANCH_ELEMENTS)
    );
}

#[test]
fn streamed_nested_absmax_counts_body_operations_and_reduction_comparisons() {
    let platform = simulate_timetable(&absmax_timetable());

    let pe = platform.pe("pe0").unwrap();
    let reduction_chunks = ABSMAX_ELEMENTS.div_ceil(ABSMAX_CHUNK);
    // A is read once; the scalar Neg is reread for each reduction chunk.
    let read_bytes = (ABSMAX_ELEMENTS + reduction_chunks) * FP32_BYTES;
    let comparisons = ABSMAX_ELEMENTS - 1;
    assert_eq!(pe.memory_traffic_bytes(), (read_bytes, FP32_BYTES));
    assert_eq!(
        pe.machine_ops().by_operation.get("maximum"),
        Some(&ABSMAX_ELEMENTS)
    );
    assert_eq!(
        pe.machine_ops().by_operation.get("mul"),
        Some(&ABSMAX_ELEMENTS)
    );
    assert_eq!(
        pe.machine_ops().by_operation.get("compare"),
        Some(&comparisons)
    );
    assert!(pe.peak_sram_bytes() <= SRAM_BYTES);
}

#[test]
fn nested_absmax_excludes_tail_elements_from_arithmetic_and_comparisons() {
    let platform = simulate_timetable(&absmax_tail_timetable());

    let pe = platform.pe("pe0").unwrap();
    let chunks_per_block = ABSMAX_BLOCK_ELEMENTS.div_ceil(ABSMAX_TAIL_CHUNK);
    // Neg is read even for an excluded tail chunk; only valid A elements do
    // work.
    let scalar_reads = ABSMAX_BLOCKS * chunks_per_block;
    let read_bytes = (ABSMAX_ELEMENTS + scalar_reads) * FP32_BYTES;
    // Each nonempty block needs one fewer comparison than its valid elements.
    let comparisons = ABSMAX_ELEMENTS - ABSMAX_BLOCKS;
    assert_eq!(
        pe.memory_traffic_bytes(),
        (read_bytes, ABSMAX_BLOCKS * FP32_BYTES)
    );
    assert_eq!(
        pe.machine_ops().by_operation.get("maximum"),
        Some(&ABSMAX_ELEMENTS)
    );
    assert_eq!(
        pe.machine_ops().by_operation.get("mul"),
        Some(&ABSMAX_ELEMENTS)
    );
    assert_eq!(
        pe.machine_ops().by_operation.get("compare"),
        Some(&comparisons)
    );
}
