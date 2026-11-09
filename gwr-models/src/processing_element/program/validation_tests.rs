// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use yaml_serde::from_str;

use super::executable::Expr;
use crate::processing_element::program::syntax::{ProgramSpec, ReductionOp};
use crate::processing_element::program::validate::{ProgramBindings, validate_program};
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{Tensor, TensorView};

const SHARED_AXIS_ELEMENTS: usize = 8;
const SHARED_AXIS_VALUE_BYTES: u64 = (SHARED_AXIS_ELEMENTS * size_of::<f32>()) as u64;

fn invalid_matrix_mac_error(expr: &str) -> String {
    let expr = expr.trim().replace('\n', "\n      ");
    let source = format!(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [m, n]
    expr:
      {expr}
"
    );
    let program: ProgramSpec = from_str(&source).unwrap();
    validate_program(
        &program,
        ProgramBindings {
            inputs: vec![
                view(&[2, 3], DataType::Fp32, 0),
                view(&[3, 2], DataType::Fp32, 128),
            ],
            outputs: vec![view(&[2, 2], DataType::Fp32, 256)],
        },
    )
    .unwrap_err()
    .0
}

fn shared_axis_program() -> ProgramSpec {
    from_str(
        r"
inputs: [A, B]
outputs: [Y, Z]
assignments:
  - output: Y
    axes: [i]
    expr:
      read: { tensor: A, indices: [i] }
  - output: Z
    axes: [i]
    expr:
      read: { tensor: B, indices: [i] }
",
    )
    .unwrap()
}

fn shared_axis_bindings(output_elements: usize) -> ProgramBindings {
    ProgramBindings {
        inputs: vec![
            view(&[1], DataType::Fp32, 0),
            view(
                &[SHARED_AXIS_ELEMENTS],
                DataType::Fp32,
                SHARED_AXIS_VALUE_BYTES,
            ),
        ],
        outputs: vec![
            view(
                &[output_elements],
                DataType::Fp32,
                2 * SHARED_AXIS_VALUE_BYTES,
            ),
            view(
                &[SHARED_AXIS_ELEMENTS],
                DataType::Fp32,
                3 * SHARED_AXIS_VALUE_BYTES,
            ),
        ],
    }
}

fn view(shape: &[usize], dtype: DataType, addr: u64) -> TensorView {
    TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
}

#[test]
fn allows_unused_declared_inputs() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      read: { tensor: A, indices: [i] }
",
    )
    .unwrap();
    validate_program(
        &program,
        ProgramBindings {
            inputs: vec![
                view(&[4], DataType::Fp32, 0),
                view(&[4], DataType::Fp32, 32),
            ],
            outputs: vec![view(&[4], DataType::Fp32, 64)],
        },
    )
    .unwrap();
}

#[test]
fn validates_max_reduction_over_a_temporary() {
    let program: ProgramSpec = from_str(
        r"
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
",
    )
    .unwrap();
    let region = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Fp32, 64)],
        },
    )
    .unwrap();
    assert_eq!(region.value("T").unwrap().shape, [2, 3]);
}

#[test]
fn validates_a_sum_reduction_over_an_affine_window() {
    let program: ProgramSpec = from_str(
        r"
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
",
    )
    .unwrap();
    validate_program(
        &program,
        ProgramBindings {
            inputs: vec![
                view(&[3], DataType::Fp32, 0),
                view(&[3], DataType::Fp32, 32),
            ],
            outputs: vec![view(&[3], DataType::Fp32, 64)],
        },
    )
    .unwrap();
}

#[test]
fn assignment_requires_output_or_indices_output() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
assignments:
  - axes: [i]
    expr:
      read: { tensor: A, indices: [i] }
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[4], DataType::Fp32, 0)],
            outputs: vec![view(&[4], DataType::Fp32, 64)],
        },
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("must name output or indices_output"),
        "{error}"
    );
}

#[test]
fn named_calls_require_lowercase_operation_names() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      Sin:
        - read: { tensor: A, indices: [i] }
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[4], DataType::Fp32, 0)],
            outputs: vec![view(&[4], DataType::Fp32, 64)],
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("lowercase"), "{error}");
}

#[test]
fn matrix_mac_rejects_a_missing_reduction_axis() {
    let error = invalid_matrix_mac_error(
        r"
matrix_mac:
  accumulator: fp32
  args:
    - read: { tensor: A, indices: [m, k] }
    - read: { tensor: B, indices: [k, n] }
",
    );
    assert!(error.contains("one reduction axis"), "{error}");
}

#[test]
fn matrix_mac_rejects_a_single_operand() {
    let error = invalid_matrix_mac_error(
        r"
matrix_mac:
  accumulator: fp32
  over: [k]
  args:
    - read: { tensor: A, indices: [m, k] }
",
    );
    assert!(error.contains("two direct reads"), "{error}");
}

#[test]
fn matrix_mac_rejects_excluded_operand_bounds() {
    let error = invalid_matrix_mac_error(
        r"
matrix_mac:
  accumulator: fp32
  over: [k]
  args:
    - read: { tensor: A, indices: [m, k], bounds: exclude }
    - read: { tensor: B, indices: [k, n] }
",
    );
    assert!(
        error.contains("operands must directly index the reduction axis"),
        "{error}"
    );
}

#[test]
fn matrix_mac_requires_both_operands_to_share_the_reduction_axis() {
    let error = invalid_matrix_mac_error(
        r"
matrix_mac:
  accumulator: fp32
  over: [k]
  args:
    - read: { tensor: A, indices: [m, k] }
    - read: { tensor: B, indices: [m, n] }
",
    );
    assert!(error.contains("reduction axis"), "{error}");
}

#[test]
fn index_only_argmin_uses_declared_output_dtype() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [I]
assignments:
  - indices_output: I
    axes: [i]
    expr:
      reduce:
        op: min
        over: [k]
        value:
          read: { tensor: A, indices: [i, k] }
",
    )
    .unwrap();
    let region = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Int32, 128)],
        },
    )
    .unwrap();
    assert_eq!(region.value("I").unwrap().shape, [2]);
    assert_eq!(region.value("I").unwrap().dtype, DataType::Int32);
    assert!(matches!(
        &region.program.assignments[0].expr,
        Expr::Reduce {
            op: ReductionOp::Min,
            ..
        }
    ));
}

#[test]
fn index_result_allows_multiple_reduction_dimensions() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [I]
assignments:
  - indices_output: I
    axes: [i]
    expr:
      reduce:
        op: max
        over: [k, j]
        value:
          read: { tensor: A, indices: [i, k, j] }
",
    )
    .unwrap();
    let region = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[2, 3, 4], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Int64, 128)],
        },
    )
    .unwrap();
    assert_eq!(region.value("I").unwrap().shape, [2]);
}

#[test]
fn index_result_requires_min_or_max_reduction() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [I]
assignments:
  - indices_output: I
    axes: [i]
    expr:
      reduce:
        op: sum
        over: [k]
        value:
          read: { tensor: A, indices: [i, k] }
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Int64, 128)],
        },
    )
    .unwrap_err();
    assert!(error.0.contains("indices_output requires"), "{error:?}");
}

#[test]
fn reports_input_binding_mismatches() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      add:
        - read: { tensor: A, indices: [i] }
        - read: { tensor: B, indices: [i] }
",
    )
    .unwrap();
    let input = view(&[4], DataType::Fp32, 0);
    let output = view(&[4], DataType::Fp32, 64);

    let input_error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![input],
            outputs: vec![output],
        },
    )
    .unwrap_err();
    assert_eq!(
        input_error.0,
        "program input bindings differ: declared 2, bound 1"
    );
}

#[test]
fn reports_output_binding_mismatches() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      add:
        - read: { tensor: A, indices: [i] }
        - read: { tensor: B, indices: [i] }
",
    )
    .unwrap();
    let input = view(&[4], DataType::Fp32, 0);

    let output_error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![input.clone(), input],
            outputs: vec![],
        },
    )
    .unwrap_err();
    assert_eq!(
        output_error.0,
        "program output bindings differ: declared 1, bound 0"
    );
}

#[test]
fn validates_broadcasted_elementwise_shape() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [C]
assignments:
  - output: C
    axes: [m, n]
    expr:
      add:
        - read: { tensor: A, indices: [m, n] }
        - read: { tensor: B, indices: [m, n] }
",
    )
    .unwrap();
    let region = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![
                view(&[7, 1], DataType::Fp32, 0),
                view(&[1, 5], DataType::Fp32, 64),
            ],
            outputs: vec![view(&[7, 5], DataType::Fp32, 128)],
        },
    )
    .unwrap();
    assert_eq!(region.extent("m"), Some(7));
    assert_eq!(region.extent("n"), Some(5));
    assert_eq!(region.value("C").unwrap().shape, [7, 5]);
}

#[test]
fn broadcast_output_shape_is_independent_of_assignment_order() {
    let bindings = shared_axis_bindings(SHARED_AXIS_ELEMENTS);

    // The read from B establishes i for both independent assignments.
    for reverse_assignments in [false, true] {
        let mut program = shared_axis_program();
        if reverse_assignments {
            program.assignments.reverse();
        }
        let region = validate_program(&program, bindings.clone()).unwrap();
        assert_eq!(region.value("Y").unwrap().shape, [SHARED_AXIS_ELEMENTS]);
    }
}

#[test]
fn rejects_singleton_output_when_another_assignment_expands_its_axis() {
    let bindings = shared_axis_bindings(1);

    // Y must cover the complete logical axis, even though A broadcasts.
    for reverse_assignments in [false, true] {
        let mut program = shared_axis_program();
        if reverse_assignments {
            program.assignments.reverse();
        }
        let error = validate_program(&program, bindings.clone()).unwrap_err();
        assert!(
            error.0.contains('Y') && error.0.contains("shape"),
            "{error}"
        );
    }
}

#[test]
fn temporary_shape_uses_axis_extents_from_later_independent_assignments() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y, Z]
temporaries:
  T: { dtype: fp32 }
assignments:
  - output: T
    axes: [i]
    expr:
      read: { tensor: A, indices: [i] }
  - output: Y
    axes: [i]
    expr:
      read: { tensor: T, indices: [i] }
  - output: Z
    axes: [i]
    expr:
      read: { tensor: B, indices: [i] }
",
    )
    .unwrap();
    let bindings = shared_axis_bindings(SHARED_AXIS_ELEMENTS);

    // T inherits the full axis before its consumer is validated.
    for reverse_assignments in [false, true] {
        let mut program = program.clone();
        if reverse_assignments {
            program.assignments.reverse();
        }
        let region = validate_program(&program, bindings.clone()).unwrap();
        assert_eq!(region.value("T").unwrap().shape, [SHARED_AXIS_ELEMENTS]);
    }
}

#[test]
fn rejects_different_index_mappings_for_the_same_input_within_an_assignment() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [i, j]
    expr:
      add:
        - read: { tensor: A, indices: [i, j] }
        - read: { tensor: A, indices: [j, i] }
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[4, 4], DataType::Fp32, 0)],
            outputs: vec![view(&[4, 4], DataType::Fp32, 128)],
        },
    )
    .unwrap_err();
    assert!(error.0.contains("multiple index mappings"), "{error:?}");
}

#[test]
fn ordinary_affine_reads_must_stay_inside_the_tensor() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
extents: { i: 4 }
assignments:
  - output: Y
    axes: [i]
    expr:
      add:
        - read:
            tensor: A
            indices:
              - affine:
                  terms: { i: 1 }
                  offset: 1
        - read: { tensor: B, indices: [i] }
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![
                view(&[4], DataType::Fp32, 0),
                view(&[4], DataType::Fp32, 64),
            ],
            outputs: vec![view(&[4], DataType::Fp32, 128)],
        },
    )
    .unwrap_err();
    assert!(error.0.contains("exceeds its dimension"), "{error:?}");
}

#[test]
fn rejects_incompatible_extent_with_tensor_context() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [C]
assignments:
  - output: C
    axes: [m, n]
    expr:
      add:
        - read: { tensor: A, indices: [m, n] }
        - read: { tensor: B, indices: [m, n] }
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![
                view(&[7, 3], DataType::Fp32, 0),
                view(&[7, 5], DataType::Fp32, 128),
            ],
            outputs: vec![view(&[7, 5], DataType::Fp32, 256)],
        },
    )
    .unwrap_err();
    assert!(error.0.contains('B'), "{error:?}");
    assert!(error.0.contains('n'), "{error:?}");
}

#[test]
fn validates_contraction_and_internal_storage_dtype() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
temporaries: { T: { dtype: fp32 } }
assignments:
  - output: T
    axes: [m, n]
    expr:
      reduce:
        op: sum
        over: [k]
        value:
          mul:
            - read: { tensor: A, indices: [m, k] }
            - read: { tensor: B, indices: [k, n] }
  - output: Y
    axes: [m, n]
    expr:
      add:
        - read: { tensor: T, indices: [m, n] }
        - read: { tensor: T, indices: [m, n] }
",
    )
    .unwrap();
    let region = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![
                view(&[2, 4], DataType::Fp16, 0),
                view(&[4, 3], DataType::Bf16, 128),
            ],
            outputs: vec![view(&[2, 3], DataType::Fp32, 256)],
        },
    )
    .unwrap();
    assert_eq!(region.value("T").unwrap().shape, [2, 3]);
    assert_eq!(region.value("T").unwrap().dtype, DataType::Fp32);
    assert_eq!(region.extent("k"), Some(4));
}

#[test]
fn index_result_uses_declared_tensor_dtype() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y, I]
extents: { o: 2, k: 2 }
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
                  offset: 0
            bounds: exclude
",
    )
    .unwrap();
    let region = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[4], DataType::Fp32, 0)],
            outputs: vec![
                view(&[2], DataType::Fp32, 128),
                view(&[2], DataType::Int32, 256),
            ],
        },
    )
    .unwrap();
    assert_eq!(region.value("I").unwrap().dtype, DataType::Int32);
}

#[test]
fn rejects_an_affine_window_with_no_valid_elements() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
extents: { o: 1, k: 2 }
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
                  terms: { o: 2, k: 1 }
                  offset: 10
            bounds: exclude
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[4], DataType::Fp32, 0)],
            outputs: vec![view(&[1], DataType::Fp32, 128)],
        },
    )
    .unwrap_err();
    assert!(
        error.0.contains("empty") || error.0.contains("valid"),
        "{error:?}"
    );
}

#[test]
fn rejects_nested_reductions_during_validation() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [m]
    expr:
      reduce:
        op: sum
        over: [k]
        value:
          reduce:
            op: sum
            over: [j]
            value:
              read: { tensor: A, indices: [m, k, j] }
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[2, 3, 4], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Fp32, 128)],
        },
    )
    .unwrap_err();
    assert!(error.0.contains("nested reduction"), "{error:?}");
}

#[test]
fn rejects_unbound_read_axis() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [m]
    expr:
      add:
        - read: { tensor: A, indices: [m, n] }
        - read: { tensor: A, indices: [m, n] }
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Fp32, 128)],
        },
    )
    .unwrap_err();
    assert!(
        error.0.contains('n') && error.0.contains("unbound"),
        "{error:?}"
    );
}

#[test]
fn rejects_shifted_internal_dependency() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y]
temporaries: { T: { dtype: fp32 } }
extents: { i: 8 }
assignments:
  - output: T
    axes: [i]
    expr:
      add:
        - read: { tensor: A, indices: [i] }
        - read: { tensor: A, indices: [i] }
  - output: Y
    axes: [i]
    expr:
      add:
        - read:
            tensor: T
            indices:
              - affine:
                  terms: { i: 1 }
                  offset: 1
        - read: { tensor: A, indices: [i] }
",
    )
    .unwrap();
    let error = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![view(&[8], DataType::Fp32, 0)],
            outputs: vec![view(&[8], DataType::Fp32, 128)],
        },
    )
    .unwrap_err();
    assert!(
        error.0.contains("cross-tile") || error.0.contains("internal"),
        "{error:?}"
    );
}

#[test]
fn infers_output_shape_for_nested_scalar_arithmetic() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      add:
        - mul:
            - read: { tensor: A, indices: [i] }
            - read: { tensor: B, indices: [i] }
        - read: { tensor: A, indices: [i] }
",
    )
    .unwrap();
    let region = validate_program(
        &program,
        ProgramBindings {
            inputs: vec![
                view(&[8], DataType::Fp32, 0),
                view(&[8], DataType::Fp32, 64),
            ],
            outputs: vec![view(&[8], DataType::Fp32, 128)],
        },
    )
    .unwrap();
    assert_eq!(region.value("Y").unwrap().shape, [8]);
}

#[test]
fn validates_constant_affine_row_indices_against_tensor_bounds() {
    for offset in [-1, 0, 1, 2] {
        let program: ProgramSpec = from_str(&format!(
            r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [k]
    expr:
      read:
        tensor: A
        indices:
          - affine:
              terms: {{}}
              offset: {offset}
          - k
"
        ))
        .unwrap();
        let result = validate_program(
            &program,
            ProgramBindings {
                inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
                outputs: vec![view(&[3], DataType::Fp32, 64)],
            },
        );
        assert_eq!(
            result.is_ok(),
            (0..2).contains(&offset),
            "constant affine offset {offset}: {result:?}"
        );
    }
}

#[test]
fn scalar_call_rejects_a_reduction_child_at_raw_validation() {
    let program: ProgramSpec = from_str("inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: []\n   expr: {exp: [{reduce: {op: sum, over: [k], value: {read: {tensor: A, indices: [k]}}}}]}\n").unwrap();
    let bindings = ProgramBindings {
        inputs: vec![view(&[2], DataType::Fp32, 0)],
        outputs: vec![view(&[], DataType::Fp32, 128)],
    };
    assert!(validate_program(&program, bindings).is_err());
}
