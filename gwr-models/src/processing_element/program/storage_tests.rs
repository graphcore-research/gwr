// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use yaml_serde::from_str;

use super::prepared::{PreparedProgram, Schedule};
use crate::processing_element::program::plan::allocate_temporaries;
use crate::processing_element::program::storage::{
    ReductionStateBinding, select_reduction_state_binding_for_optional_output,
};
use crate::processing_element::program::syntax::ProgramSpec;
use crate::processing_element::program::tile::plan_tiles;
use crate::processing_element::program::validate::{ProgramBindings, validate_program};
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{Tensor, TensorView};

#[test]
fn reduction_state_binding_cases() {
    use DataType::{Bool, Fp8, Fp16, Fp32, Int64};
    use ReductionStateBinding::{Destination, EngineTransient, Scratch};
    for (name, (inputs, output, elements, persistent), expected) in [
        (
            "transient",
            (&[Fp16][..], Some(Fp16), 16, false),
            EngineTransient,
        ),
        (
            "same_width_destination",
            (&[Fp16][..], Some(Fp16), 16, true),
            Destination,
        ),
        (
            "wider_destination",
            (&[Fp16][..], Some(Fp32), 16, true),
            Destination,
        ),
        (
            "narrower_scratch",
            (&[Fp16][..], Some(Fp8), 2, true),
            Scratch { bytes: 4 },
        ),
        (
            "packed_no_output",
            (&[Bool][..], None, 9, true),
            Scratch { bytes: 2 },
        ),
        (
            "widest_operand",
            (&[Fp16, Int64][..], Some(Fp16), 4, true),
            Scratch { bytes: 32 },
        ),
    ] {
        assert_eq!(
            select_reduction_state_binding_for_optional_output(
                inputs, output, elements, persistent
            )
            .unwrap(),
            expected,
            "{name}"
        );
    }
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
        ProgramBindings {
            inputs: vec![view(0)],
            outputs: vec![view(128)],
        },
    )
    .unwrap();
    let tile = plan_tiles(&region, None).unwrap().tiles().next().unwrap();
    let program = PreparedProgram::from_facts(region, None).unwrap();
    let Schedule::Flat { group, .. } = &program.schedule else {
        unreachable!()
    };
    let allocation = allocate_temporaries(program.group(group), &tile).unwrap();
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
        ProgramBindings {
            inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
            outputs: vec![
                view(&[2], DataType::Fp32, 128),
                view(&[2], DataType::Int64, 256),
            ],
        },
    )
    .unwrap();
    let tile = plan_tiles(&region, None).unwrap().tiles().next().unwrap();
    let program = PreparedProgram::from_facts(region, None).unwrap();
    let Schedule::Flat { group, .. } = &program.schedule else {
        unreachable!()
    };
    let allocation = allocate_temporaries(program.group(group), &tile).unwrap();
    assert!(
        allocation
            .slots
            .iter()
            .any(|slot| slot.values.contains(&"I".to_owned()))
    );
}
