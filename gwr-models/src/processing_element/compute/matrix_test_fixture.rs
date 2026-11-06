// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use super::{
    CapabilityConfig, CapabilityKind, ComputeWork, MatrixServiceTile, MatrixWorkShape,
    OperationClass,
};
use crate::processing_element::tensors::dtype::DataType;

pub(super) fn matrix_capability(
    ops_per_tick: f64,
    accumulator: DataType,
    tile: Option<MatrixServiceTile>,
) -> CapabilityConfig {
    CapabilityConfig {
        class: "matrix".into(),
        input_dtypes: Some(vec![vec![DataType::Bf16; 2]]),
        ops_per_tick,
        kind: CapabilityKind::Matrix {
            accumulator,
            matrix_service_tile: tile,
        },
    }
}

pub(super) fn matrix_operation_classes(flops_per_op: usize) -> BTreeMap<String, OperationClass> {
    BTreeMap::from([(
        "matrix".into(),
        OperationClass {
            operations: vec!["matrix_mac".into()],
            arity: 2,
            flops_per_op,
        },
    )])
}

pub(super) fn matrix_work(accumulator: DataType, shape: MatrixWorkShape) -> ComputeWork {
    ComputeWork::matrix_mac([DataType::Bf16; 2], accumulator, shape).unwrap()
}
