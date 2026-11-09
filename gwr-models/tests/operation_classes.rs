// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::compute::{
    ComputeConfig, ComputeModel, ComputeWork, OperationClass,
};
use gwr_models::processing_element::tensors::dtype::DataType;

#[test]
fn one_class_rate_serializes_distinct_operations_on_one_engine() {
    let classes: BTreeMap<String, OperationClass> =
        yaml_serde::from_str("trig: { operations: [sin, cos, tan], arity: 1, flops_per_op: 1 }")
            .unwrap();
    let config: ComputeConfig = yaml_serde::from_str(
        "engines: { vector: { capabilities: [{ class: trig, input_dtypes: [[fp32]], ops_per_tick: 32 }] } }",
    )
    .unwrap();
    let model = ComputeModel::new(&config, &classes).unwrap();
    let plan = model
        .lower(&ComputeWork::parallel(vec![
            ComputeWork::operation("sin", vec![DataType::Fp32], 32),
            ComputeWork::operation("cos", vec![DataType::Fp32], 32),
        ]))
        .unwrap();
    assert_eq!(plan.service_ticks().unwrap(), 2);
    assert_eq!(plan.stages[0].len(), 1);
    assert_eq!(plan.stages[0][0].machine_ops.by_operation["sin"], 32);
    assert_eq!(plan.stages[0][0].machine_ops.by_operation["cos"], 32);
    assert_eq!(plan.stages[0][0].machine_ops.flop_equivalents, 64);
}

#[test]
fn weighted_flops_do_not_change_machine_operation_total() {
    let counts = MachineOpCounts::single("matrix_mac", 6, 2).unwrap();
    assert_eq!(counts.checked_operation_total().unwrap(), 6);
    assert_eq!(counts.flop_equivalents(), 12);
}
