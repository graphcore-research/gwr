// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::compute::{
    ComputeConfig, ComputeModel, ComputeWork, OperationClass,
};
use gwr_models::processing_element::tensors::dtype::DataType;

#[test]
fn compact_class_capabilities_round_trip() {
    let yaml = "engines: { vector: { capabilities: [{ class: math, input_dtypes: [[fp16, fp16], [bf16, bf16]], ops_per_tick: 200 }] } }";
    let config: ComputeConfig = yaml_serde::from_str(yaml).unwrap();
    ComputeModel::new(&config, &classes()).unwrap();
    let emitted = yaml_serde::to_string(&config).unwrap();
    assert!(emitted.contains("class: math"));
    assert_eq!(config, yaml_serde::from_str(&emitted).unwrap());
}

#[test]
fn exact_combinations_and_fallback_rate_are_preserved() {
    let yaml = "engines: { vector: { capabilities: [{ class: math, ops_per_tick: 10 }, { class: math, input_dtypes: [[fp16, bf16]], ops_per_tick: 100 }] } }";
    let config: ComputeConfig = yaml_serde::from_str(yaml).unwrap();
    let model = ComputeModel::new(&config, &classes()).unwrap();
    for (inputs, ticks) in [
        (vec![DataType::Fp16, DataType::Bf16], 1),
        (vec![DataType::Bf16, DataType::Fp16], 10),
    ] {
        assert_eq!(
            model
                .lower(&ComputeWork::operation("add", inputs, 100))
                .unwrap()
                .service_ticks()
                .unwrap(),
            ticks
        );
    }
}

#[test]
fn invalid_class_declarations_are_rejected() {
    for invalid in [
        "engines: { vector: { capabilities: [{ class: missing, ops_per_tick: 100 }] } }",
        "engines: { vector: { capabilities: [{ class: math, input_dtypes: [[fp32]], ops_per_tick: 100 }] } }",
        "engines: { vector: { capabilities: [{ class: math, ops_per_tick: 0 }] } }",
    ] {
        let config: ComputeConfig = yaml_serde::from_str(invalid).unwrap();
        assert!(ComputeModel::new(&config, &classes()).is_err());
    }
    assert!(
        yaml_serde::from_str::<ComputeConfig>(
            "engines: { vector: { capabilities: [{ op: add, ops_per_tick: 100 }] } }"
        )
        .is_err()
    );
}

fn classes() -> BTreeMap<String, OperationClass> {
    yaml_serde::from_str("math: { operations: [add, mul], arity: 2 }").unwrap()
}
