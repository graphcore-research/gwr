// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::compute::{
    CapabilityConfig, ComputeConfig, ComputeModel, ComputeWork, EngineConfig, OperationClass,
    TypedWork, WorkStage,
};
use gwr_models::processing_element::tensors::dtype::DataType;

#[test]
fn explicit_datatype_rate_overrides_class_fallback() {
    let classes = BTreeMap::from([("math".into(), class(&["add", "mul"], 2, 1))]);
    let model = model(
        classes,
        &[(
            "alu",
            vec![
                capability("math", None, 10.0),
                capability("math", Some(vec![vec![DataType::Fp16; 2]]), 100.0),
            ],
        )],
    );
    let work = ComputeWork {
        stages: vec![WorkStage {
            operations: vec![
                TypedWork::operation("add", vec![DataType::Fp16; 2], 100),
                TypedWork::operation("mul", vec![DataType::Fp32; 2], 100),
            ],
        }],
    };
    let plan = model.lower(&work).unwrap();
    assert_eq!(plan.service_ticks().unwrap(), 11);
    assert_eq!(plan.stages[0].len(), 1);
    assert_eq!(plan.stages[0][0].machine_ops.by_operation["add"], 100);
    assert_eq!(plan.stages[0][0].machine_ops.by_operation["mul"], 100);
}

#[test]
fn independent_engines_overlap() {
    let classes = BTreeMap::from([
        ("trig".into(), class(&["sin", "cos"], 1, 1)),
        ("math".into(), class(&["add"], 2, 1)),
    ]);
    let two_engines = model(
        classes,
        &[
            ("trig", vec![capability("trig", None, 10.0)]),
            ("vector", vec![capability("math", None, 10.0)]),
        ],
    );
    let plan = two_engines
        .lower(&ComputeWork::parallel(vec![
            ComputeWork::operation("sin", vec![DataType::Fp32], 10),
            ComputeWork::operation("add", vec![DataType::Fp32; 2], 10),
        ]))
        .unwrap();
    assert_eq!(plan.service_ticks().unwrap(), 1);
    assert_eq!(plan.stages[0].len(), 2);
}

#[test]
fn equivalent_engines_are_available_to_the_same_operation() {
    let classes = BTreeMap::from([("trig".into(), class(&["sin", "cos"], 1, 1))]);
    let ambiguous = model(
        classes,
        &[
            ("a", vec![capability("trig", None, 10.0)]),
            ("b", vec![capability("trig", None, 20.0)]),
        ],
    );
    let plan = ambiguous
        .lower(&ComputeWork::operation("sin", vec![DataType::Fp32], 20))
        .unwrap();
    assert_eq!(plan.stages[0][0].alternatives.len(), 2);
    assert_eq!(plan.stages[0][0].alternatives[0].1, 1);
    let triple = ambiguous
        .lower(&ComputeWork::parallel(vec![
            ComputeWork::operation("sin", vec![DataType::Fp32], 20),
            ComputeWork::operation("sin", vec![DataType::Fp32], 20),
            ComputeWork::operation("sin", vec![DataType::Fp32], 20),
        ]))
        .unwrap();
    assert_eq!(triple.service_ticks().unwrap(), 2);
}

#[test]
fn class_arity_and_missing_operation_are_checked() {
    let classes = BTreeMap::from([("predicate".into(), class(&["is_nan"], 1, 1))]);
    let model = model(
        classes,
        &[(
            "alu",
            vec![capability(
                "predicate",
                Some(vec![vec![DataType::Fp32]]),
                8.0,
            )],
        )],
    );
    assert!(
        model
            .lower(&ComputeWork::operation("is_nan", vec![DataType::Fp32], 4))
            .is_ok()
    );
    for work in [
        ComputeWork::operation("is_nan", vec![DataType::Fp32; 2], 4),
        ComputeWork::operation("sin", vec![DataType::Fp32], 4),
    ] {
        let error = model.lower(&work).unwrap_err().to_string();
        assert!(error.contains("unsupported capability"), "{error}");
    }
}

#[test]
fn malformed_class_and_capability_are_rejected() {
    let valid = class(&["sin"], 1, 1);
    let config = |cap| ComputeConfig {
        engines: BTreeMap::from([(
            "alu".into(),
            EngineConfig {
                matrix_accumulator_capacity_bytes: None,
                capabilities: vec![cap],
            },
        )]),
    };
    assert!(
        ComputeModel::new(&config(capability("missing", None, 1.0)), &BTreeMap::new()).is_err()
    );
    for invalid in [
        capability("trig", None, 0.0),
        capability("trig", None, f64::NAN),
        capability("trig", Some(vec![vec![DataType::Fp32; 2]]), 1.0),
        capability("trig", Some(vec![]), 1.0),
    ] {
        assert!(
            ComputeModel::new(
                &config(invalid),
                &BTreeMap::from([("trig".into(), valid.clone())])
            )
            .is_err()
        );
    }
    let duplicate = class(&["sin", "sin"], 1, 1);
    assert!(
        ComputeModel::new(
            &config(capability("trig", None, 1.0)),
            &BTreeMap::from([("trig".into(), duplicate)])
        )
        .is_err()
    );
    let mixed_case = class(&["Sin"], 1, 1);
    let error = ComputeModel::new(
        &config(capability("trig", None, 1.0)),
        &BTreeMap::from([("trig".into(), mixed_case)]),
    )
    .unwrap_err();
    assert!(error.to_string().contains("lowercase"), "{error}");
}

#[test]
fn matrix_mac_uses_declared_capability() {
    let classes = BTreeMap::from([
        ("matrix".into(), class(&["matrix_mac"], 2, 2)),
        ("mul".into(), class(&["mul"], 2, 1)),
        ("add".into(), class(&["add"], 2, 1)),
    ]);
    let work = ComputeWork::operation("matrix_mac", vec![DataType::Fp32; 2], 6);
    let accelerated = model(
        classes.clone(),
        &[("matrix", vec![capability("matrix", None, 3.0)])],
    )
    .lower(&work)
    .unwrap();
    assert_eq!(accelerated.service_ticks().unwrap(), 2);
    assert_eq!(
        accelerated.stages[0][0].machine_ops.by_operation["matrix_mac"],
        6
    );
    assert_eq!(accelerated.stages[0][0].machine_ops.flop_equivalents, 12);
}

fn capability(class: &str, inputs: Option<Vec<Vec<DataType>>>, rate: f64) -> CapabilityConfig {
    CapabilityConfig {
        matrix_service_tile: None,
        execution: None,
        class: class.into(),
        input_dtypes: inputs,
        ops_per_tick: rate,
    }
}

fn class(operations: &[&str], arity: usize, flops_per_op: usize) -> OperationClass {
    OperationClass {
        operations: operations.iter().map(|name| (*name).into()).collect(),
        arity,
        flops_per_op,
    }
}

fn model(
    classes: BTreeMap<String, OperationClass>,
    engines: &[(&str, Vec<CapabilityConfig>)],
) -> ComputeModel {
    let config = ComputeConfig {
        engines: engines
            .iter()
            .map(|(name, caps)| {
                (
                    (*name).into(),
                    EngineConfig {
                        matrix_accumulator_capacity_bytes: None,
                        capabilities: caps.clone(),
                    },
                )
            })
            .collect(),
    };
    ComputeModel::new(&config, &classes).unwrap()
}
