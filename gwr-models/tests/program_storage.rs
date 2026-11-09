// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::types::SimError;
use gwr_models::processing_element::program::plan::allocate_temporaries;
use gwr_models::processing_element::program::storage::{
    ReductionStateBinding, reduction_state_binding_for_operands,
    reduction_state_binding_for_optional_output,
};
use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::program::tile::plan_tiles;
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};

#[test]
fn single_matrix_batch_needs_no_sram_reduction_state() {
    assert_eq!(
        reduction_state_binding(DataType::Fp16, DataType::Fp16, 16, false).unwrap(),
        ReductionStateBinding::EngineTransient
    );
}

#[test]
fn persistent_result_uses_destination_when_it_is_as_wide_as_the_input() {
    assert_eq!(
        reduction_state_binding(DataType::Fp16, DataType::Fp16, 16, true).unwrap(),
        ReductionStateBinding::Destination
    );
}

#[test]
fn persistent_fp32_result_uses_destination_slot() {
    assert_eq!(
        reduction_state_binding(DataType::Fp16, DataType::Fp32, 16, true).unwrap(),
        ReductionStateBinding::Destination
    );
}

#[test]
fn persistent_integer_result_uses_destination_when_it_is_wider_than_the_input() {
    assert_eq!(
        reduction_state_binding(DataType::Int8, DataType::Int32, 2, true).unwrap(),
        ReductionStateBinding::Destination
    );
}

#[test]
fn scratch_uses_input_width_when_output_is_narrower() {
    assert_eq!(
        reduction_state_binding(DataType::Fp16, DataType::Fp8, 2, true).unwrap(),
        ReductionStateBinding::Scratch { bytes: 4 }
    );
}

#[test]
fn reduction_without_output_uses_input_width() {
    assert_eq!(
        reduction_state_binding_for_optional_output(&[DataType::Bool], None, 9, true).unwrap(),
        ReductionStateBinding::Scratch { bytes: 2 }
    );
}

#[test]
fn mixed_width_operands_size_the_partial_for_the_widest_input() {
    assert_eq!(
        reduction_state_binding_for_operands(
            &[DataType::Fp16, DataType::Int64],
            DataType::Fp16,
            4,
            true
        )
        .unwrap(),
        ReductionStateBinding::Scratch { bytes: 32 }
    );
}

#[test]
fn diamond_keeps_source_live_through_both_consumers_then_reuses_its_slot() {
    let program: ProgramSpec = yaml_serde::from_str(
        r"
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
    expr: { mul: [{ read: { tensor: T0, indices: [i] } }, { read: { tensor: A, indices: [i] } }] }
  - output: T3
    axes: [i]
    expr: { add: [{ read: { tensor: T1, indices: [i] } }, { read: { tensor: A, indices: [i] } }] }
  - output: Y
    axes: [i]
    expr: { add: [{ read: { tensor: T3, indices: [i] } }, { read: { tensor: T2, indices: [i] } }] }
",
    )
    .unwrap();
    let view = |addr| TensorView::new_full(Tensor::new(&[4], &DataType::Fp32, addr).unwrap());
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(0)],
            outputs: vec![view(128)],
        },
    )
    .unwrap();
    let tile = plan_tiles(&region, None).unwrap().tiles().next().unwrap();
    let allocation = allocate_temporaries(&region, &tile).unwrap();
    assert_eq!(allocation.arena_bytes, 48);
    assert!(
        allocation
            .slots
            .iter()
            .any(|slot| slot.values == ["T0", "T3"])
    );
    assert!(allocation.slots.iter().any(|slot| slot.values == ["T1"]));
    assert!(allocation.slots.iter().any(|slot| slot.values == ["T2"]));
}

#[test]
fn dual_result_reduction_allocates_local_index_result() {
    let program: ProgramSpec = yaml_serde::from_str(
        r"
inputs: [A]
outputs: [Y, Z]
temporaries: { I: { dtype: int64 } }
assignments:
  - output: Y
    indices_output: I
    axes: [i]
    expr: { reduce: { op: max, over: [k], value: { read: { tensor: A, indices: [i, k] } } } }
  - output: Z
    axes: [i]
    expr: { add: [{ read: { tensor: I, indices: [i] } }, { read: { tensor: I, indices: [i] } }] }
",
    )
    .unwrap();
    let view = |shape: &[usize], dtype, addr| {
        TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
    };
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
            outputs: vec![
                view(&[2], DataType::Fp32, 128),
                view(&[2], DataType::Int64, 256),
            ],
        },
    )
    .unwrap();
    let tile = plan_tiles(&region, None).unwrap().tiles().next().unwrap();
    let allocation = allocate_temporaries(&region, &tile).unwrap();
    assert!(
        allocation
            .slots
            .iter()
            .any(|slot| slot.values.contains(&"I".to_owned()))
    );
}

fn reduction_state_binding(
    input: DataType,
    output: DataType,
    output_elements: usize,
    persistent: bool,
) -> Result<ReductionStateBinding, SimError> {
    reduction_state_binding_for_operands(&[input], output, output_elements, persistent)
}
