// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::collections::BTreeMap;

use yaml_serde::{from_str, to_string};

use crate::processing_element::compute::{
    ComputeConfig, ComputeModel, ComputeWork, MatrixWorkShape, OperationClass,
};
use crate::processing_element::estimate_peak_tile_sram;
use crate::processing_element::tensors::dtype::DataType;

const OUTPUT_ROWS: usize = 2;
const OUTPUT_COLUMNS: usize = 2;
const REDUCTION_ELEMENTS: usize = 8;
const MATRIX_MACS: usize = OUTPUT_ROWS * OUTPUT_COLUMNS * REDUCTION_ELEMENTS;

fn matrix_accumulator_compute_config() -> ComputeConfig {
    from_str(
        r"
execution_units:
  matrix:
    capabilities:
      - { class: matrix, input_dtypes: [[bf16, bf16]], ops_per_tick: 4, accumulator: fp32 }
      - { class: matrix, input_dtypes: [[bf16, bf16]], ops_per_tick: 8, accumulator: bf16 }
",
    )
    .unwrap()
}

fn matrix_operation_classes() -> BTreeMap<String, OperationClass> {
    from_str("matrix: { operations: [matrix_mac], arity: 2 }").unwrap()
}

fn matrix_work_with_accumulator(inputs: Vec<DataType>, accumulator: DataType) -> ComputeWork {
    ComputeWork::matrix_mac(
        inputs,
        accumulator,
        MatrixWorkShape {
            batches: 1,
            m: OUTPUT_ROWS,
            n: OUTPUT_COLUMNS,
            k: REDUCTION_ELEMENTS,
        },
    )
    .unwrap()
}

#[test]
fn matrix_accumulator_dtype_selects_the_matching_capability() {
    let model = ComputeModel::new(
        &matrix_accumulator_compute_config(),
        &matrix_operation_classes(),
    )
    .unwrap();
    for (accumulator, macs_per_tick) in [(DataType::Fp32, 4), (DataType::Bf16, 8)] {
        let work = matrix_work_with_accumulator(vec![DataType::Bf16; 2], accumulator);
        let plan = model.lower(&work).unwrap();
        assert_eq!(
            plan.service_ticks().unwrap(),
            MATRIX_MACS.div_ceil(macs_per_tick) as u64
        );
    }
}

#[test]
fn matrix_work_rejects_an_unsupported_accumulator_dtype() {
    let model = ComputeModel::new(
        &matrix_accumulator_compute_config(),
        &matrix_operation_classes(),
    )
    .unwrap();
    let work = matrix_work_with_accumulator(vec![DataType::Bf16; 2], DataType::Int32);
    assert!(
        model
            .lower(&work)
            .unwrap_err()
            .0
            .contains("unsupported capability")
    );
}

#[test]
fn matrix_work_rejects_unsupported_operand_dtypes() {
    let model = ComputeModel::new(
        &matrix_accumulator_compute_config(),
        &matrix_operation_classes(),
    )
    .unwrap();
    assert!(
        model
            .lower(&matrix_work_with_accumulator(
                vec![DataType::Fp16; 2],
                DataType::Fp32
            ))
            .unwrap_err()
            .0
            .contains("unsupported capability")
    );
}

#[test]
fn scalar_capability_declaration_rejects_an_accumulator() {
    let scalar_classes = from_str("matrix: { operations: [add], arity: 2 }").unwrap();
    assert!(
        ComputeModel::new(&matrix_accumulator_compute_config(), &scalar_classes)
            .unwrap_err()
            .0
            .contains("accumulator is only supported for matrix_mac")
    );
}

#[test]
fn matrix_accumulator_configuration_round_trips() {
    let config = matrix_accumulator_compute_config();
    let emitted = to_string(&config).unwrap();
    assert!(emitted.contains("accumulator: bf16"));
    assert_eq!(config, from_str(&emitted).unwrap());
}

#[test]
fn chunked_matrix_sram_estimate_includes_fp32_partials_and_fp16_output() {
    use crate::processing_element::program::syntax::{ProgramSpec, TileSpec};
    use crate::processing_element::tensors::dtype::DataType;
    use crate::processing_element::tensors::{Tensor, TensorView};
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
 - output: Y
   axes: [m, n]
   expr:
     matrix_mac:
       accumulator: fp32
       over: [k]
       args:
         - read: { tensor: A, indices: [m, k] }
         - read: { tensor: B, indices: [k, n] }
",
    )
    .unwrap();
    let view = |shape: &[usize], addr| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp16, addr).unwrap())
    };
    let bytes = estimate_peak_tile_sram(
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
fn single_chunk_matrix_sram_estimate_includes_fp32_accumulator_and_fp16_output() {
    use crate::processing_element::program::syntax::{ProgramSpec, TileSpec};
    use crate::processing_element::tensors::dtype::DataType;
    use crate::processing_element::tensors::{Tensor, TensorView};
    let program: ProgramSpec = from_str(
        r"
inputs: [A,B]
outputs: [Y]
assignments:
 - output: Y
   axes: [m,n]
   expr:
     matrix_mac:
       accumulator: fp32
       over: [k]
       args:
         - read: { tensor: A, indices: [m, k] }
         - read: { tensor: B, indices: [k, n] }
",
    )
    .unwrap();
    let view = |shape: &[usize], addr| {
        TensorView::new_full(Tensor::new(shape, &DataType::Fp16, addr).unwrap())
    };
    assert_eq!(
        estimate_peak_tile_sram(
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
