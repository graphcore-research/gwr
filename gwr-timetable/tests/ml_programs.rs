// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};

#[test]
fn rmsnorm_has_a_row_statistic_and_external_scale() {
    // Row-wise RMS normalization. Scalar inputs carry epsilon and reciprocal
    // row length without introducing memory-sized constant tensors.
    let p: ProgramSpec = yaml_serde::from_str(
        r"
inputs: [A, W, Inv, Eps]
outputs: [Y]
temporaries:
  S: {dtype: fp32}
  R: {dtype: fp32}
assignments:
  - output: S
    axes: [r]
    expr: {reduce: {op: sum, over: [k], value: {mul: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: A, indices: [r,k]}}]}}}
  - output: R
    axes: [r]
    expr: {rsqrt: [{add: [{mul: [{read: {tensor: S, indices: [r]}}, {read: {tensor: Inv, indices: []}}]}, {read: {tensor: Eps, indices: []}}]}]}
  - output: Y
    axes: [r,k]
    expr: {mul: [{mul: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: R, indices: [r]}}]}, {read: {tensor: W, indices: [k]}}]}
",
    )
    .unwrap();
    let region = validate_program(
        &p,
        &RegionBindings {
            inputs: vec![
                view(&[1, 1024], 0),
                view(&[1024], 8192),
                view(&[], 16384),
                view(&[], 16400),
            ],
            outputs: vec![view(&[1, 1024], 32768)],
        },
    )
    .unwrap();
    assert_eq!(region.extent("k"), Some(1024));
    assert_eq!(p.inputs, ["A", "W", "Inv", "Eps"]);
    assert!(format!("{p:?}").contains("rsqrt"));
}

#[test]
fn stable_softmax_reduces_a_row_before_normalizing_it() {
    // Stable softmax. The selected tile strategy determines whether row
    // statistics are recomputed and whether score chunks are reread; counts
    // remain observable.
    let p: ProgramSpec = yaml_serde::from_str(
        r"
inputs: [A]
outputs: [Y]
temporaries: {M: {dtype: fp32}, S: {dtype: fp32}}
assignments:
  - output: M
    axes: [r]
    expr: {reduce: {op: max, over: [k], value: {read: {tensor: A, indices: [r,k]}}}}
  - output: S
    axes: [r]
    expr: {reduce: {op: sum, over: [k], value: {exp: [{sub: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: M, indices: [r]}}]}]}}}
  - output: Y
    axes: [r,k]
    expr: {div: [{exp: [{sub: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: M, indices: [r]}}]}]}, {read: {tensor: S, indices: [r]}}]}
",
    )
    .unwrap();
    assert!(format!("{p:?}").contains("Max"));
    assert!(format!("{p:?}").contains("exp"));
    assert!(format!("{p:?}").contains("div"));
    validate_program(
        &p,
        &RegionBindings {
            inputs: vec![view(&[2, 32], 0)],
            outputs: vec![view(&[2, 32], 1024)],
        },
    )
    .unwrap();
}

fn view(shape: &[usize], addr: u64) -> TensorView {
    TensorView::new_full(Tensor::new(shape, &DataType::Fp32, addr).unwrap())
}
