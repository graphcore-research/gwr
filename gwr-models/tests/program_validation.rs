// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::program::syntax::{ExprSpec, ProgramSpec, ReductionOp};
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};

#[test]
fn unused_declared_input_can_establish_a_dependency() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: { read: { tensor: A, indices: [i] } }\n",
    ).unwrap();
    validate_program(
        &program,
        &RegionBindings {
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
fn fused_max_reduction_can_consume_a_temporary() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y]\ntemporaries: { T: { dtype: fp32 } }\nassignments:\n  - output: T\n    axes: [o, k]\n    expr: { add: [{ read: { tensor: A, indices: [o, k] } }, { read: { tensor: A, indices: [o, k] } }] }\n  - output: Y\n    axes: [o]\n    expr: { reduce: { op: max, over: [k], value: { read: { tensor: T, indices: [o, k] } } } }\n",
    ).unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Fp32, 64)],
        },
    )
    .unwrap();
    assert_eq!(region.value("T").unwrap().shape, [2, 3]);
}

#[test]
fn sum_reduction_can_read_an_affine_window() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [Y]\nextents: { y: 3, k: 3 }\nassignments:\n  - output: Y\n    axes: [y]\n    expr: { reduce: { op: sum, over: [k], value: { mul: [{ read: { tensor: A, indices: [{ affine: { terms: { y: 1, k: 1 }, offset: -1 } }], bounds: exclude } }, { read: { tensor: B, indices: [k] } }] } } }\n",
    ).unwrap();
    validate_program(
        &program,
        &RegionBindings {
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
fn parses_value_and_indices_outputs() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y, I]\nassignments:\n  - output: Y\n    indices_output: I\n    axes: [o]\n    expr: { reduce: { op: max, over: [k], value: { read: { tensor: A, indices: [o, k] } } } }\n",
    )
    .unwrap();
    assert_eq!(program.assignments[0].output.as_deref(), Some("Y"));
    assert_eq!(program.assignments[0].indices_output.as_deref(), Some("I"));
}

#[test]
fn assignment_requires_output_or_indices_output() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y]\nassignments:\n  - axes: [i]\n    expr: { read: { tensor: A, indices: [i] } }\n",
    )
    .unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
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
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: { Sin: { args: [{ read: { tensor: A, indices: [i] } }] } }\n",
    )
    .unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[4], DataType::Fp32, 0)],
            outputs: vec![view(&[4], DataType::Fp32, 64)],
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("lowercase"), "{error}");
}

#[test]
fn matrix_mac_requires_two_reads_sharing_one_reduction_axis() {
    for (expr, expected) in [
        (
            "{ matrix_mac: { args: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] } }",
            "one reduction axis",
        ),
        (
            "{ matrix_mac: { over: [k], args: [{ read: { tensor: A, indices: [m, k] } }] } }",
            "two direct reads",
        ),
        (
            "{ matrix_mac: { over: [k], args: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [m, n] } }] } }",
            "reduction axis",
        ),
    ] {
        let source = format!(
            "inputs: [A, B]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [m, n]\n    expr: {expr}\n"
        );
        let program: ProgramSpec = yaml_serde::from_str(&source).unwrap();
        let error = validate_program(
            &program,
            &RegionBindings {
                inputs: vec![
                    view(&[2, 3], DataType::Fp32, 0),
                    view(&[3, 2], DataType::Fp32, 128),
                ],
                outputs: vec![view(&[2, 2], DataType::Fp32, 256)],
            },
        )
        .unwrap_err();
        assert!(error.0.contains(expected), "{error:?}");
    }
}

#[test]
fn index_only_argmin_uses_declared_output_dtype() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [I]\nassignments:\n  - indices_output: I\n    axes: [i]\n    expr: { reduce: { op: min, over: [k], value: { read: { tensor: A, indices: [i, k] } } } }\n",
    )
    .unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Int32, 128)],
        },
    )
    .unwrap();
    assert_eq!(region.value("I").unwrap().shape, [2]);
    assert_eq!(region.value("I").unwrap().dtype, DataType::Int32);
    assert!(matches!(
        &region.program.assignments[0].expr,
        ExprSpec::Reduce {
            op: ReductionOp::Min,
            ..
        }
    ));
}

#[test]
fn index_result_allows_multiple_reduction_dimensions() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [I]\nassignments:\n  - indices_output: I\n    axes: [i]\n    expr: { reduce: { op: max, over: [k, j], value: { read: { tensor: A, indices: [i, k, j] } } } }\n",
    )
    .unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[2, 3, 4], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Int64, 128)],
        },
    )
    .unwrap();
    assert_eq!(region.value("I").unwrap().shape, [2]);
}

#[test]
fn index_result_requires_min_or_max_reduction() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [I]\nassignments:\n  - indices_output: I\n    axes: [i]\n    expr: { reduce: { op: sum, over: [k], value: { read: { tensor: A, indices: [i, k] } } } }\n",
    )
    .unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[2, 3], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Int64, 128)],
        },
    )
    .unwrap_err();
    assert!(error.0.contains("indices_output requires"), "{error:?}");
}

#[test]
fn reports_input_and_output_binding_mismatches_separately() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }\n",
    )
    .unwrap();
    let input = view(&[4], DataType::Fp32, 0);
    let output = view(&[4], DataType::Fp32, 64);

    let input_error = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![input.clone()],
            outputs: vec![output.clone()],
        },
    )
    .unwrap_err();
    assert_eq!(
        input_error.0,
        "program input bindings differ: declared 2, bound 1"
    );

    let output_error = validate_program(
        &program,
        &RegionBindings {
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
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [C]\nassignments:\n  - output: C\n    axes: [m, n]\n    expr: { add: [{ read: { tensor: A, indices: [m, n] } }, { read: { tensor: B, indices: [m, n] } }] }\n",
    ).unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
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
fn rejects_two_index_mappings_for_one_boundary_input() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i, j]\n    expr: { add: [{ read: { tensor: A, indices: [i, j] } }, { read: { tensor: A, indices: [j, i] } }] }\n",
    ).unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[4, 4], DataType::Fp32, 0)],
            outputs: vec![view(&[4, 4], DataType::Fp32, 128)],
        },
    )
    .unwrap_err();
    assert!(error.0.contains("multiple index mappings"), "{error:?}");
}

#[test]
fn ordinary_affine_reads_must_stay_inside_the_tensor() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [Y]\nextents: { i: 4 }\nassignments:\n  - output: Y\n    axes: [i]\n    expr: { add: [{ read: { tensor: A, indices: [{ affine: { terms: { i: 1 }, offset: 1 } }] } }, { read: { tensor: B, indices: [i] } }] }\n",
    ).unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
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
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [C]\nassignments:\n  - output: C\n    axes: [m, n]\n    expr: { add: [{ read: { tensor: A, indices: [m, n] } }, { read: { tensor: B, indices: [m, n] } }] }\n",
    ).unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
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
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [Y]\ntemporaries: { T: { dtype: fp32 } }\nassignments:\n  - output: T\n    axes: [m, n]\n    expr: { reduce: { op: sum, over: [k], value: { mul: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] } } }\n  - output: Y\n    axes: [m, n]\n    expr: { add: [{ read: { tensor: T, indices: [m, n] } }, { read: { tensor: T, indices: [m, n] } }] }\n",
    ).unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
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
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y, I]\nextents: { o: 2, k: 2 }\nassignments:\n  - output: Y\n    indices_output: I\n    axes: [o]\n    expr: { reduce: { op: max, over: [k], value: { read: { tensor: A, indices: [{ affine: { terms: { o: 2, k: 1 }, offset: 0 } }], bounds: exclude } } } }\n",
    ).unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
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
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y]\nextents: { o: 1, k: 2 }\nassignments:\n  - output: Y\n    axes: [o]\n    expr: { reduce: { op: max, over: [k], value: { read: { tensor: A, indices: [{ affine: { terms: { o: 2, k: 1 }, offset: 10 } }], bounds: exclude } } } }\n",
    ).unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
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
fn rejects_nested_reductions_before_execution() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [m]\n    expr: { reduce: { op: sum, over: [k], value: { reduce: { op: sum, over: [j], value: { read: { tensor: A, indices: [m, k, j] } } } } } }\n",
    ).unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
            inputs: vec![view(&[2, 3, 4], DataType::Fp32, 0)],
            outputs: vec![view(&[2], DataType::Fp32, 128)],
        },
    )
    .unwrap_err();
    assert!(error.0.contains("nested reduction"), "{error:?}");
}

#[test]
fn rejects_unbound_read_axis() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [m]\n    expr: { add: [{ read: { tensor: A, indices: [m, n] } }, { read: { tensor: A, indices: [m, n] } }] }\n",
    ).unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
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
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y]\ntemporaries: { T: { dtype: fp32 } }\nextents: { i: 8 }\nassignments:\n  - output: T\n    axes: [i]\n    expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: A, indices: [i] } }] }\n  - output: Y\n    axes: [i]\n    expr: { add: [{ read: { tensor: T, indices: [{ affine: { terms: { i: 1 }, offset: 1 } }] } }, { read: { tensor: A, indices: [i] } }] }\n",
    ).unwrap();
    let error = validate_program(
        &program,
        &RegionBindings {
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
fn validates_nested_scalar_arithmetic_before_memory_access() {
    let program: ProgramSpec = yaml_serde::from_str(
        "inputs: [A, B]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: { add: [{ mul: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }, { read: { tensor: A, indices: [i] } }] }\n",
    ).unwrap();
    let region = validate_program(
        &program,
        &RegionBindings {
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
fn constant_affine_indices_select_nonsingleton_rows_and_validate_bounds() {
    for offset in [-1, 0, 1, 2] {
        let program:ProgramSpec=yaml_serde::from_str(&format!(r"
inputs: [A]
outputs: [Y]
assignments:
 - output: Y
   axes: [k]
   expr: {{ read: {{ tensor: A, indices: [{{ affine: {{ terms: {{}}, offset: {offset} }} }}, k] }} }}
")).unwrap();
        let result = validate_program(
            &program,
            &RegionBindings {
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

fn view(shape: &[usize], dtype: DataType, addr: u64) -> TensorView {
    TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
}
