// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::program::scalar_dtype::validate_homogeneous_scalar_assignments;
use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::program::validate::{
    RegionBindings, ValidatedRegion, validate_program,
};
use gwr_models::processing_element::tensors::dtype::DataType as D;
use gwr_models::processing_element::tensors::{Tensor, TensorView};

fn scalar(expr: &str, input: D, output: D) -> ValidatedRegion {
    let spec: ProgramSpec = yaml_serde::from_str(&format!(
        "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: {expr}\n"
    ))
    .unwrap();
    validate_program(
        &spec,
        &RegionBindings {
            inputs: vec![view(&[1], input, 0)],
            outputs: vec![view(&[1], output, 128)],
        },
    )
    .unwrap()
}

fn view(shape: &[usize], dtype: D, address: u64) -> TensorView {
    TensorView::new_full(Tensor::new(shape, &dtype, address).unwrap())
}

const READ: &str = "{ read: { tensor: A, indices: [i] } }";

#[test]
fn default_validation_preserves_implicit_dtype_transitions_but_guard_rejects_them() {
    for expr in [
        READ.to_owned(),
        format!("{{ mul: [{READ}, {{ mul: [{READ}, {READ}] }}] }}"),
        format!("{{ convert: [{{ mul: [{READ}, {READ}] }}] }}"),
    ] {
        let region = scalar(&expr, D::Fp32, D::Bf16);
        let error = validate_homogeneous_scalar_assignments(&region, &["Y"]).unwrap_err();
        assert!(
            error.0.contains('Y') && error.0.contains("Fp32") && error.0.contains("Bf16"),
            "{error}"
        );
    }
}

#[test]
fn explicit_fp32_temporary_and_final_convert_preserve_source_opmath_contract() {
    let spec: ProgramSpec = yaml_serde::from_str(
        "inputs: [A]\noutputs: [Y]\ntemporaries: { A32: { dtype: fp32 }, Mixed32: { dtype: fp32 } }\nassignments:\n  - output: A32\n    axes: [i]\n    expr: { convert: [{ read: { tensor: A, indices: [i] } }] }\n  - output: Mixed32\n    axes: [i]\n    expr: { mul: [{ mul: [{ read: { tensor: A32, indices: [i] } }, { read: { tensor: A32, indices: [i] } }] }, { read: { tensor: A32, indices: [i] } }] }\n  - output: Y\n    axes: [i]\n    expr: { convert: [{ read: { tensor: Mixed32, indices: [i] } }] }\n"
    ).unwrap();
    let region = validate_program(
        &spec,
        &RegionBindings {
            inputs: vec![view(&[2], D::Bf16, 0)],
            outputs: vec![view(&[2], D::Bf16, 128)],
        },
    )
    .unwrap();
    validate_homogeneous_scalar_assignments(&region, &["A32", "Mixed32", "Y"]).unwrap();
}

#[test]
fn convert_and_reinterpret_have_distinct_width_contracts() {
    for (input, output, accepted) in [
        (D::Fp32, D::Int32, true),
        (D::Fp32, D::Int16, false),
        (D::Fp4, D::Uint8, false),
    ] {
        let region = scalar(&format!("{{ reinterpret: [{READ}] }}"), input, output);
        assert_eq!(
            validate_homogeneous_scalar_assignments(&region, &["Y"]).is_ok(),
            accepted
        );
        let converted = scalar(&format!("{{ convert: [{READ}] }}"), input, output);
        validate_homogeneous_scalar_assignments(&converted, &["Y"]).unwrap();
    }
}

#[test]
fn cast_calls_require_one_operand_only_in_opt_in_guard() {
    for op in ["convert", "reinterpret"] {
        let region = scalar(&format!("{{ {op}: [{READ}, {READ}] }}"), D::Fp32, D::Fp32);
        assert!(
            validate_homogeneous_scalar_assignments(&region, &["Y"])
                .unwrap_err()
                .0
                .contains("one operand")
        );
    }
}

#[test]
fn selection_rejects_empty_unknown_duplicate_and_ambiguous_writers() {
    let mut region = scalar(READ, D::Fp32, D::Fp32);
    for outputs in [vec![], vec!["missing"], vec!["Y", "Y"]] {
        assert!(validate_homogeneous_scalar_assignments(&region, &outputs).is_err());
    }
    region
        .program
        .assignments
        .push(region.program.assignments[0].clone());
    assert!(
        validate_homogeneous_scalar_assignments(&region, &["Y"])
            .unwrap_err()
            .0
            .contains("multiple writers")
    );
}

#[test]
fn publicly_mutated_region_with_unknown_read_returns_contextual_error() {
    let mut region = scalar(READ, D::Fp32, D::Fp32);
    region.values.remove("A");
    let error = validate_homogeneous_scalar_assignments(&region, &["Y"]).unwrap_err();
    assert!(
        error.0.contains("assignment Y") && error.0.contains("unknown scalar dtype tensor A"),
        "{error}"
    );
}

#[test]
fn unselected_mixed_signature_is_untouched() {
    let spec: ProgramSpec = yaml_serde::from_str(&format!(
        "inputs: [A]\noutputs: [Y, T]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: {{ not_equal: [{READ}, {READ}] }}\n  - output: T\n    axes: [i]\n    expr: {READ}\n"
    )).unwrap();
    let region = validate_program(
        &spec,
        &RegionBindings {
            inputs: vec![view(&[2], D::Uint32, 0)],
            outputs: vec![view(&[2], D::Bool, 128), view(&[2], D::Uint32, 256)],
        },
    )
    .unwrap();
    validate_homogeneous_scalar_assignments(&region, &["T"]).unwrap();
    assert!(validate_homogeneous_scalar_assignments(&region, &["Y"]).is_err());
}

#[test]
fn selected_matrix_and_reduction_contracts_are_explicitly_unsupported() {
    for expr in [
        "{ reduce: { op: sum, over: [k], value: { read: { tensor: A, indices: [i, k] } } } }",
        "{ matrix_mac: { over: [k], args: [{ read: { tensor: A, indices: [i, k] } }, { read: { tensor: B, indices: [k, j] } }] } }",
    ] {
        let matrix = expr.contains("matrix_mac");
        let source = format!(
            "inputs: {}\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: {}\n    expr: {expr}\n",
            if matrix { "[A, B]" } else { "[A]" },
            if matrix { "[i, j]" } else { "[i]" }
        );
        let spec: ProgramSpec = yaml_serde::from_str(&source).unwrap();
        let region = validate_program(
            &spec,
            &RegionBindings {
                inputs: if matrix {
                    vec![view(&[2, 3], D::Fp32, 0), view(&[3, 2], D::Fp32, 256)]
                } else {
                    vec![view(&[2, 3], D::Fp32, 0)]
                },
                outputs: vec![view(if matrix { &[2, 2] } else { &[2] }, D::Fp32, 512)],
            },
        )
        .unwrap();
        assert!(
            validate_homogeneous_scalar_assignments(&region, &["Y"])
                .unwrap_err()
                .0
                .contains("unsupported")
        );
    }
}
