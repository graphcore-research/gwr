// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::estimate_peak_tile_sram;
use gwr_models::processing_element::program::SRAM_SLOT_ALIGNMENT_BYTES;
use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::task::ProgramTaskConfig;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use yaml_serde::from_str;

const ELEMENTS: usize = 9;
const CHUNK: usize = 4;
const FP32_BYTES: usize = 4;
const SCALAR_SLOT_BYTES: usize = FP32_BYTES.next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);
const ROW_SLOT_BYTES: usize = (ELEMENTS * FP32_BYTES).next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);

fn reduction_pipeline_task(assignments: &str, output_shape: &[usize]) -> ProgramTaskConfig {
    let program: ProgramSpec = from_str(&format!(
        r"
inputs: [A]
outputs: [Y]
temporaries: {{ T: {{ dtype: fp32 }} }}
assignments:
{assignments}
"
    ))
    .unwrap();
    let view = |shape: &[usize], address| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp32, address).unwrap())
    };
    ProgramTaskConfig {
        id: "automatic_phases".into(),
        program,
        tiling: Some(TileSpec {
            sizes: BTreeMap::from([("i".into(), CHUNK)]),
        }),
        inputs: vec![view(&[ELEMENTS], 0)],
        outputs: vec![view(output_shape, 128)],
    }
}

fn estimated_peak_sram_bytes(task: &ProgramTaskConfig) -> usize {
    estimate_peak_tile_sram(
        &task.program,
        &task.inputs,
        &task.outputs,
        task.tiling.as_ref().unwrap(),
    )
    .unwrap()
}

#[test]
fn broadcast_after_reduction_plans_two_input_passes_and_a_retained_scalar() {
    let task = reduction_pipeline_task(
        r"
 - output: T
   axes: []
   expr:
     reduce:
       op: sum
       over: [i]
       value:
         read: { tensor: A, indices: [i] }
 - output: Y
   axes: [i]
   expr:
     add:
       - read: { tensor: A, indices: [i] }
       - read: { tensor: T, indices: [] }
",
        &[ELEMENTS],
    );
    assert_eq!(
        task.planned_traffic_bytes().unwrap(),
        (ELEMENTS * FP32_BYTES * 2, ELEMENTS * FP32_BYTES)
    );
    // The output phase retains the scalar alongside one input/output chunk.
    assert_eq!(
        estimated_peak_sram_bytes(&task),
        CHUNK * FP32_BYTES * 2 + SCALAR_SLOT_BYTES
    );
}

#[test]
fn reduction_of_a_local_tensor_reserves_the_complete_producer() {
    let task = reduction_pipeline_task(
        r"
 - output: T
   axes: [i]
   expr:
     exp:
       - read: { tensor: A, indices: [i] }
 - output: Y
   axes: []
   expr:
     reduce:
       op: sum
       over: [i]
       value:
         read: { tensor: T, indices: [i] }
",
        &[],
    );
    assert_eq!(
        task.planned_traffic_bytes().unwrap(),
        (ELEMENTS * FP32_BYTES, FP32_BYTES)
    );
    // Nine FP32 values reserve 40 aligned bytes, plus the producer input chunk.
    assert_eq!(
        estimated_peak_sram_bytes(&task),
        ROW_SLOT_BYTES + CHUNK * FP32_BYTES
    );
}

#[test]
fn matching_free_axes_reserve_chunk_sized_pipeline_temporary_storage() {
    let task = reduction_pipeline_task(
        r"
 - output: T
   axes: [i]
   expr:
     exp:
       - read: { tensor: A, indices: [i] }
 - output: Y
   axes: [i]
   expr:
     add:
       - read: { tensor: T, indices: [i] }
       - read: { tensor: T, indices: [i] }
",
        &[ELEMENTS],
    );
    assert_eq!(
        task.planned_traffic_bytes().unwrap(),
        (ELEMENTS * FP32_BYTES, ELEMENTS * FP32_BYTES)
    );
    assert_eq!(estimated_peak_sram_bytes(&task), CHUNK * FP32_BYTES * 3);
}

#[test]
fn matching_free_axes_plan_one_boundary_input_pass() {
    let task = reduction_pipeline_task(
        r"
 - output: T
   axes: [i]
   expr:
     exp:
       - read: { tensor: A, indices: [i] }
 - output: Y
   axes: [i]
   expr:
     add:
       - read: { tensor: T, indices: [i] }
       - read: { tensor: A, indices: [i] }
",
        &[ELEMENTS],
    );
    assert_eq!(
        task.planned_traffic_bytes().unwrap(),
        (ELEMENTS * FP32_BYTES, ELEMENTS * FP32_BYTES)
    );
}

#[test]
fn independent_reduction_preserves_chunk_sized_pipeline_storage() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [S, Y]
temporaries: {T: {dtype: fp32}}
assignments:
 - output: T
   axes: [i]
   expr:
     read: { tensor: A, indices: [i] }
 - output: S
   axes: []
   expr:
     reduce:
       op: sum
       over: [j]
       value:
         read: { tensor: B, indices: [j] }
 - output: Y
   axes: [i]
   expr:
     read: { tensor: T, indices: [i] }
",
    )
    .unwrap();
    let view = |shape: &[usize], address| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp32, address).unwrap())
    };
    let tiling = TileSpec {
        sizes: BTreeMap::from([("i".into(), 64)]),
    };
    let bytes = estimate_peak_tile_sram(
        &program,
        &[view(&[1025], 0), view(&[4], 8192)],
        &[view(&[], 12288), view(&[1025], 16384)],
        &tiling,
    )
    .unwrap();
    // The input, temporary and output each occupy one 64-element chunk.
    assert_eq!(bytes, 768);
}
