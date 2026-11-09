// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::collections::BTreeMap;

use gwr_models::processing_element::compute::{
    ComputeConfig, ComputeModel, ComputeWork, OperationClass, TypedWork, WorkStage,
};
#[test]
fn same_storage_selects_explicit_dense_or_sparse_instruction() {
    let config: ComputeConfig = yaml_serde::from_str(r"
engines:
  matrix:
    capabilities:
      - { class: matrix, input_dtypes: [[fp16, fp16]], ops_per_tick: 4, execution: { instruction: tensor, accumulator: fp32, sparsity: dense } }
      - { class: matrix, input_dtypes: [[fp16, fp16]], ops_per_tick: 8, execution: { instruction: tensor, accumulator: fp32, sparsity: structured } }
").unwrap();
    let classes: BTreeMap<String, OperationClass> =
        yaml_serde::from_str("matrix: { operations: [matrix_mac], arity: 2 }").unwrap();
    let model = ComputeModel::new(&config, &classes).unwrap();
    for (sparsity, ticks) in [("dense", 8), ("structured", 4)] {
        let demand: TypedWork = yaml_serde::from_str(&format!("signature: {{ op: matrix_mac, inputs: [fp16, fp16], execution: {{ instruction: tensor, accumulator: fp32, sparsity: {sparsity} }} }}\ncount: 32")).unwrap();
        assert_eq!(
            model
                .lower(&ComputeWork {
                    stages: vec![WorkStage {
                        operations: vec![demand]
                    }]
                })
                .unwrap()
                .service_ticks()
                .unwrap(),
            ticks
        );
    }
    let demand: TypedWork =
        yaml_serde::from_str("signature: { op: matrix_mac, inputs: [fp16, fp16] }\ncount: 32")
            .unwrap();
    assert!(
        model
            .lower(&ComputeWork {
                stages: vec![WorkStage {
                    operations: vec![demand]
                }]
            })
            .is_err()
    );
}
#[test]
fn fp32_matrix_partials_have_independent_storage_from_fp16_destination() {
    use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
    use gwr_models::processing_element::tensors::dtype::DataType;
    use gwr_models::processing_element::tensors::{Tensor, TensorView};
    let program: ProgramSpec = yaml_serde::from_str(r"
inputs: [A, B]
outputs: [Y]
assignments:
 - output: Y
   axes: [m, n]
   expr: { matrix_mac: { execution: { instruction: tensor, accumulator: fp32 }, over: [k], args: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] } }
").unwrap();
    let view = |shape: &[usize], addr| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp16, addr).unwrap())
    };
    let bytes = gwr_models::processing_element::estimate_peak_tile_sram(
        &program,
        &[view(&[2, 8], 0), view(&[8, 2], 64)],
        &[view(&[2, 2], 128)],
        &TileSpec {
            sizes: BTreeMap::from([("k".into(), 4)]),
        },
    )
    .unwrap();
    // Two 16-byte input chunks + 8-byte destination + 16-byte FP32 state.
    assert_eq!(bytes, 56);
}
#[test]
fn single_matrix_chunk_keeps_accumulator_live_until_separate_output_cast() {
    use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
    use gwr_models::processing_element::tensors::dtype::DataType;
    use gwr_models::processing_element::tensors::{Tensor, TensorView};
    let program:ProgramSpec=yaml_serde::from_str(r"
inputs: [A,B]
outputs: [Y]
assignments:
 - output: Y
   axes: [m,n]
   expr: { matrix_mac: { execution: { instruction: tensor, accumulator: fp32 }, over: [k], args: [{ read: { tensor: A, indices: [m,k] } }, { read: { tensor: B, indices: [k,n] } }] } }
").unwrap();
    let view = |shape: &[usize], addr| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp16, addr).unwrap())
    };
    assert_eq!(
        gwr_models::processing_element::estimate_peak_tile_sram(
            &program,
            &[view(&[2, 8], 0), view(&[8, 2], 64)],
            &[view(&[2, 2], 128)],
            &TileSpec {
                sizes: BTreeMap::from([("k".into(), 8)])
            }
        )
        .unwrap(),
        88
    );
}
