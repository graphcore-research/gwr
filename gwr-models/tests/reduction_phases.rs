// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::estimate_peak_tile_sram;
use gwr_models::processing_element::program::SRAM_SLOT_ALIGNMENT_BYTES;
use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::task::ProgramTaskConfig;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use yaml_serde::from_str;

const FP32_BYTES: usize = 4;
const INPUT_BASE_ADDRESS: u64 = 0;
const ROW_ELEMENTS: usize = 1025;
const REDUCTION_CHUNK_ELEMENTS: usize = 128;
const SINGLE_ROW_COUNT: usize = 1;

const ROW_BYTES: usize = ROW_ELEMENTS * FP32_BYTES;
const CHUNK_STORAGE_BYTES: usize =
    (REDUCTION_CHUNK_ELEMENTS * FP32_BYTES).next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);
const ROW_STORAGE_BYTES: usize = ROW_BYTES.next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);
const SCALAR_STORAGE_BYTES: usize = FP32_BYTES.next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);
// Put the output beyond the input row, at the next power-of-two boundary.
const SINGLE_ROW_OUTPUT_ADDRESS: u64 = INPUT_BASE_ADDRESS + ROW_BYTES.next_power_of_two() as u64;

#[test]
fn streamed_row_reduction_plans_one_reduction_pass_and_one_broadcast_pass() {
    const ROW_COUNT: usize = 2;
    const ROWS_PER_TILE: usize = 1;
    const INPUT_PASSES: usize = 2; // Reduction and broadcast.
    const BUFFERS_PER_PHASE: usize = 2; // Input and expression result/output.
    const INPUT_BYTES: usize = ROW_COUNT * ROW_BYTES;
    const OUTPUT_ADDRESS: u64 = INPUT_BASE_ADDRESS + INPUT_BYTES.next_power_of_two() as u64;

    // S[r] sums the squared values of a complete row. Y then multiplies each
    // input value by that row statistic, without materializing a squared row.
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
temporaries: { S: { dtype: fp32 } }
assignments:
 - output: S
   axes: [r]
   expr:
     reduce:
       op: sum
       over: [k]
       value:
         mul:
           - read: { tensor: A, indices: [r, k] }
           - read: { tensor: A, indices: [r, k] }

 - output: Y
   axes: [r, k]
   expr:
     mul:
       - read: { tensor: A, indices: [r, k] }
       - read: { tensor: S, indices: [r] }
",
    )
    .unwrap();

    let view = |addr| {
        TensorView::new_full(
            Tensor::new(&[ROW_COUNT, ROW_ELEMENTS], &DataType::Fp32, addr).unwrap(),
        )
    };
    let inputs = vec![view(INPUT_BASE_ADDRESS)];
    let outputs = vec![view(OUTPUT_ADDRESS)];
    let tiling = TileSpec {
        sizes: BTreeMap::from([
            ("r".into(), ROWS_PER_TILE),
            ("k".into(), REDUCTION_CHUNK_ELEMENTS),
        ]),
    };
    let task = ProgramTaskConfig {
        id: "reread".into(),
        program: program.clone(),
        tiling: Some(tiling),
        inputs,
        outputs,
    };

    // S reads each complete row once; Y reads it once more for output chunks.
    // The repeated A operands in A*A share a read within the reduction phase.
    assert_eq!(
        task.planned_traffic_bytes().unwrap(),
        (INPUT_PASSES * INPUT_BYTES, INPUT_BYTES)
    );

    // Each phase needs an input and a result buffer for the current row tile:
    // A and squared values during S, or A and output during Y. Retain one S
    // value per row, with the whole statistic tensor aligned to an SRAM slot.
    let chunk_bytes = (ROWS_PER_TILE * REDUCTION_CHUNK_ELEMENTS * FP32_BYTES)
        .next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);
    let statistic_bytes = (ROWS_PER_TILE * FP32_BYTES).next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);
    let peak_sram_bytes = estimate_peak_tile_sram(
        &program,
        &task.inputs,
        &task.outputs,
        task.tiling.as_ref().unwrap(),
    )
    .unwrap();
    assert_eq!(
        peak_sram_bytes,
        BUFFERS_PER_PHASE * chunk_bytes + statistic_bytes
    );
}

#[test]
fn stable_softmax_plans_three_input_passes_and_chunk_sized_sram() {
    const INPUT_PASSES: usize = 3; // Maximum, sum and normalization.
    const NORMALIZATION_CHUNK_BUFFERS: usize = 4; // A, Y, sub and exp.
    const ROW_STATISTICS: usize = 2; // Maximum and sum.

    // Inline exp(A-M) in S's reduction body so each chunk can be consumed
    // immediately. Only the scalar statistics M and S need local tensor slots.
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
temporaries: { M: { dtype: fp32 }, S: { dtype: fp32 } }
assignments:
 - output: M
   axes: [r]
   expr:
     reduce:
       op: max
       over: [k]
       value:
         read: { tensor: A, indices: [r, k] }

 - output: S
   axes: [r]
   expr:
     reduce:
       op: sum
       over: [k]
       value:
         exp:
           - sub:
               - read: { tensor: A, indices: [r, k] }
               - read: { tensor: M, indices: [r] }

 - output: Y
   axes: [r, k]
   expr:
     div:
       - exp:
           - sub:
               - read: { tensor: A, indices: [r, k] }
               - read: { tensor: M, indices: [r] }
       - read: { tensor: S, indices: [r] }
",
    )
    .unwrap();

    let view = |addr| {
        TensorView::new_full(
            Tensor::new(&[SINGLE_ROW_COUNT, ROW_ELEMENTS], &DataType::Fp32, addr).unwrap(),
        )
    };
    let inputs = vec![view(INPUT_BASE_ADDRESS)];
    let outputs = vec![view(SINGLE_ROW_OUTPUT_ADDRESS)];
    let tiling = TileSpec {
        sizes: BTreeMap::from([("k".into(), REDUCTION_CHUNK_ELEMENTS)]),
    };
    let task = ProgramTaskConfig {
        id: "softmax".into(),
        program: program.clone(),
        tiling: Some(tiling.clone()),
        inputs: inputs.clone(),
        outputs: outputs.clone(),
    };
    assert_eq!(
        task.planned_traffic_bytes().unwrap(),
        (INPUT_PASSES * ROW_BYTES, ROW_BYTES)
    );

    // The normalization group needs A, Y, sub and exp chunk buffers plus
    // the retained row maximum and sum. Each buffer is aligned separately.
    let peak_sram_bytes = estimate_peak_tile_sram(&program, &inputs, &outputs, &tiling).unwrap();
    let normalization_bytes = NORMALIZATION_CHUNK_BUFFERS * CHUNK_STORAGE_BYTES;
    let statistic_bytes = ROW_STATISTICS * SCALAR_STORAGE_BYTES;
    assert_eq!(peak_sram_bytes, normalization_bytes + statistic_bytes);
}

#[test]
fn materialized_local_row_sram_includes_full_row_statistic_and_output_chunk() {
    // Naming T as a local tensor makes it a materialized intermediate, unlike
    // the inline exp expression in the preceding test.
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
temporaries: { T: { dtype: fp32 }, S: { dtype: fp32 } }
assignments:
 - output: T
   axes: [r, k]
   expr:
     exp:
       - read: { tensor: A, indices: [r, k] }

 - output: S
   axes: [r]
   expr:
     reduce:
       op: sum
       over: [k]
       value:
         read: { tensor: T, indices: [r, k] }

 - output: Y
   axes: [r, k]
   expr:
     div:
       - read: { tensor: T, indices: [r, k] }
       - read: { tensor: S, indices: [r] }
",
    )
    .unwrap();

    let view = |addr| {
        TensorView::new_full(
            Tensor::new(&[SINGLE_ROW_COUNT, ROW_ELEMENTS], &DataType::Fp32, addr).unwrap(),
        )
    };
    let inputs = vec![view(INPUT_BASE_ADDRESS)];
    let outputs = vec![view(SINGLE_ROW_OUTPUT_ADDRESS)];
    let tiling = TileSpec {
        sizes: BTreeMap::from([("k".into(), REDUCTION_CHUNK_ELEMENTS)]),
    };
    let task = ProgramTaskConfig {
        id: "materialized_row".into(),
        program: program.clone(),
        tiling: Some(tiling.clone()),
        inputs: inputs.clone(),
        outputs: outputs.clone(),
    };
    assert_eq!(
        task.planned_traffic_bytes().unwrap(),
        (ROW_BYTES, ROW_BYTES)
    );
    let peak = estimate_peak_tile_sram(&program, &inputs, &outputs, &tiling).unwrap();
    // Normalization has the largest live set: retain all of T and its sum S,
    // while writing one output chunk. The earlier exp phase needs only T and
    // an input chunk, so it fits within this same reservation.
    let retained_values_bytes = ROW_STORAGE_BYTES + SCALAR_STORAGE_BYTES;
    assert_eq!(peak, retained_values_bytes + CHUNK_STORAGE_BYTES);
}

#[test]
fn different_affine_mappings_in_separate_phases_each_contribute_read_traffic() {
    const INPUT_ELEMENTS: usize = 8;
    const NEXT_INPUT_OFFSET: usize = 1;
    const OUTPUT_ELEMENTS: usize = INPUT_ELEMENTS - NEXT_INPUT_OFFSET;
    const CHUNK_ELEMENTS: usize = 3;
    const INPUT_PASSES: usize = 2; // T reads A[i]; Y reads A[i + offset].
    const INPUT_BYTES: usize = INPUT_ELEMENTS * FP32_BYTES;
    // Leave an input-sized gap between the boundary tensors.
    const OUTPUT_ADDRESS: u64 = INPUT_BASE_ADDRESS + (INPUT_BYTES * 2) as u64;

    // T[i] reads A[i], while Y[i] adds A[i+1]. Both phases read boundary A;
    // storing T locally does not make the differently mapped second read free.
    let program: ProgramSpec = from_str(&format!(
        r"
inputs: [A]
outputs: [Y]
extents: {{ i: {OUTPUT_ELEMENTS} }}
temporaries: {{ T: {{ dtype: fp32 }} }}
assignments:
 - output: T
   axes: [i]
   expr:
     read:
       tensor: A
       indices:
         - affine:
             terms: {{ i: 1 }}
             offset: 0

 - output: Y
   axes: [i]
   expr:
     add:
       - read: {{ tensor: T, indices: [i] }}
       - read:
           tensor: A
           indices:
             - affine:
                 terms: {{ i: 1 }}
                 offset: {NEXT_INPUT_OFFSET}
"
    ))
    .unwrap();

    let view = |shape: &[usize], addr| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp32, addr).unwrap())
    };
    let config = ProgramTaskConfig {
        id: "affine_phases".into(),
        program,
        tiling: Some(TileSpec {
            sizes: BTreeMap::from([("i".into(), CHUNK_ELEMENTS)]),
        }),
        inputs: vec![view(&[INPUT_ELEMENTS], INPUT_BASE_ADDRESS)],
        outputs: vec![view(&[OUTPUT_ELEMENTS], OUTPUT_ADDRESS)],
    };

    // Every output reads one element in each phase, including the final
    // partial tile. Their overlapping input ranges are separate phase reads.
    let phase_read_bytes = OUTPUT_ELEMENTS * FP32_BYTES;
    let output_bytes = OUTPUT_ELEMENTS * FP32_BYTES;
    assert_eq!(
        config.planned_traffic_bytes().unwrap(),
        (INPUT_PASSES * phase_read_bytes, output_bytes)
    );
}
