// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
#![allow(dead_code)]
use std::collections::BTreeMap;

use gwr_models::processing_element::ProcessingElementConfig;
use gwr_models::processing_element::compute::{ComputeConfig, OperationClass};
use gwr_models::processing_element::task::ProgramTaskConfig;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
pub fn config() -> ProcessingElementConfig {
    ProcessingElementConfig {
        num_active_requests: 1,
        lsu_access_bytes: 64,
        overhead_size_bytes: 8,
        sram_bytes: 1024,
        boundary_payload_service: None,
        local_payload_service: None,
        block_execution: None,
        compute: yaml_serde::from_str::<ComputeConfig>(
            "engines: {vector: {capabilities: [{class: binary, ops_per_tick: 3}]}}",
        )
        .unwrap(),
        operation_classes: yaml_serde::from_str::<BTreeMap<String, OperationClass>>(
            "binary: {operations: [add, mul, compare], arity: 2}",
        )
        .unwrap(),
    }
}
pub fn view(bytes: usize, address: u64) -> TensorView {
    TensorView::new_full(Tensor::new(&[bytes], &DataType::Int8, address).unwrap())
}
pub fn add() -> ProgramTaskConfig {
    ProgramTaskConfig {
        id: "add".into(),
        program: yaml_serde::from_str("inputs: [A, B]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: {add: [{read: {tensor: A, indices: [i]}}, {read: {tensor: B, indices: [i]}}]}\n").unwrap(),
        tiling: Some(yaml_serde::from_str("sizes: {i: 1}").unwrap()), pipeline: None,
        inputs: vec![view(4,0), view(4,100)], outputs: vec![view(4,200)],
    }
}
