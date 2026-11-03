// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::types::SimError;
use yaml_serde::from_str;

use crate::processing_element::program::plan::allocate_temporaries;
use crate::processing_element::program::storage::{
    ReductionStateBinding, select_reduction_state_binding,
    select_reduction_state_binding_for_optional_output,
};
use crate::processing_element::program::syntax::ProgramSpec;
use crate::processing_element::program::tile::plan_tiles;
use crate::processing_element::program::validate::{RegionBindings, validate_program};
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{Tensor, TensorView};

fn single_input_reduction_state_binding(
    input: DataType,
    output: DataType,
    output_elements: usize,
    persistent: bool,
) -> Result<ReductionStateBinding, SimError> {
    select_reduction_state_binding(&[input], output, output_elements, persistent)
}

#[test]
fn nonpersistent_reduction_state_uses_execution_unit_transient_storage() {
    assert_eq!(
        single_input_reduction_state_binding(DataType::Fp16, DataType::Fp16, 16, false).unwrap(),
        ReductionStateBinding::EngineTransient
    );
}

#[test]
fn persistent_reduction_state_uses_destination_when_input_and_output_widths_match() {
    assert_eq!(
        single_input_reduction_state_binding(DataType::Fp16, DataType::Fp16, 16, true).unwrap(),
        ReductionStateBinding::Destination
    );
}

#[test]
fn persistent_reduction_state_uses_destination_when_output_is_wider_than_input() {
    assert_eq!(
        single_input_reduction_state_binding(DataType::Fp16, DataType::Fp32, 16, true).unwrap(),
        ReductionStateBinding::Destination
    );
}

#[test]
fn persistent_reduction_state_uses_input_width_scratch_when_output_is_narrower() {
    assert_eq!(
        single_input_reduction_state_binding(DataType::Fp16, DataType::Fp8, 2, true).unwrap(),
        ReductionStateBinding::Scratch { bytes: 4 }
    );
}

#[test]
fn persistent_reduction_without_output_rounds_packed_scratch_up_to_whole_bytes() {
    assert_eq!(
        select_reduction_state_binding_for_optional_output(&[DataType::Bool], None, 9, true)
            .unwrap(),
        ReductionStateBinding::Scratch { bytes: 2 }
    );
}

#[test]
fn persistent_reduction_scratch_uses_the_widest_operand_width() {
    assert_eq!(
        select_reduction_state_binding(&[DataType::Fp16, DataType::Int64], DataType::Fp16, 4, true)
            .unwrap(),
        ReductionStateBinding::Scratch { bytes: 32 }
    );
}

#[test]
fn diamond_keeps_source_live_through_both_consumers_then_reuses_its_slot() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
temporaries: { T0: { dtype: fp32 }, T1: { dtype: fp32 }, T2: { dtype: fp32 }, T3: { dtype: fp32 } }
assignments:
  - output: T0
    axes: [i]
    expr:
      add:
        - read: { tensor: A, indices: [i] }
        - read: { tensor: A, indices: [i] }
  - output: T1
    axes: [i]
    expr:
      add:
        - read: { tensor: T0, indices: [i] }
        - read: { tensor: A, indices: [i] }
  - output: T2
    axes: [i]
    expr:
      mul:
        - read: { tensor: T0, indices: [i] }
        - read: { tensor: A, indices: [i] }
  - output: T3
    axes: [i]
    expr:
      add:
        - read: { tensor: T1, indices: [i] }
        - read: { tensor: A, indices: [i] }
  - output: Y
    axes: [i]
    expr:
      add:
        - read: { tensor: T3, indices: [i] }
        - read: { tensor: T2, indices: [i] }
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
fn dual_result_reduction_allocates_a_slot_for_temporary_indices() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y, Z]
temporaries: { I: { dtype: int64 } }
assignments:
  - output: Y
    indices_output: I
    axes: [i]
    expr:
      reduce:
        op: max
        over: [k]
        value:
          read: { tensor: A, indices: [i, k] }
  - output: Z
    axes: [i]
    expr:
      add:
        - read: { tensor: I, indices: [i] }
        - read: { tensor: I, indices: [i] }
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
