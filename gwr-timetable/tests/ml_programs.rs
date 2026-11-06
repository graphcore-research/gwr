// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_models::processing_element::program::PreparedProgram;
use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::program::validate::RegionBindings;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use yaml_serde::from_str;

fn view(shape: &[usize], addr: u64) -> TensorView {
    TensorView::new_full(Tensor::new(shape, &DataType::Fp32, addr).unwrap())
}

#[test]
fn validates_rmsnorm_with_row_reduction_scalar_inputs_and_external_scale() {
    // Row-wise RMS normalization. Scalar inputs carry epsilon and reciprocal
    // row length without introducing memory-sized constant tensors.
    let p: ProgramSpec = from_str(
        r"
inputs: [A, W, Inv, Eps]
outputs: [Y]
temporaries:
  S: {dtype: fp32}
  R: {dtype: fp32}
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
  - output: R
    axes: [r]
    expr:
      rsqrt:
        - add:
            - mul:
                - read: { tensor: S, indices: [r] }
                - read: { tensor: Inv, indices: [] }
            - read: { tensor: Eps, indices: [] }
  - output: Y
    axes: [r,k]
    expr:
      mul:
        - mul:
            - read: { tensor: A, indices: [r, k] }
            - read: { tensor: R, indices: [r] }
        - read: { tensor: W, indices: [k] }
",
    )
    .unwrap();
    let region = PreparedProgram::new(
        &p,
        RegionBindings {
            inputs: vec![
                view(&[1, 1024], 0),
                view(&[1024], 8192),
                view(&[], 16384),
                view(&[], 16400),
            ],
            outputs: vec![view(&[1, 1024], 32768)],
        },
        None,
    )
    .unwrap();
    assert_eq!(region.extent("k"), Some(1024));
    assert_eq!(p.inputs, ["A", "W", "Inv", "Eps"]);
    assert!(format!("{p:?}").contains("rsqrt"));
}
