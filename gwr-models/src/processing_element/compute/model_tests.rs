// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use yaml_serde::{from_str, to_string};

use crate::processing_element::compute::{
    CapabilityConfig, CapabilityKind, ComputeConfig, ComputeModel, ComputeWork,
    ExecutionUnitConfig, OperationClass, TypedWork, WorkStage,
};
use crate::processing_element::tensors::dtype::DataType;

fn predicate_model() -> ComputeModel {
    compute_model(
        &BTreeMap::from([("predicate".into(), operation_class(&["is_nan"], 1, 1))]),
        &[(
            "alu",
            vec![operation_class_capability(
                "predicate",
                Some(vec![vec![DataType::Fp32]]),
                8.0,
            )],
        )],
    )
}

fn single_capability_config(cap: CapabilityConfig) -> ComputeConfig {
    ComputeConfig {
        execution_units: BTreeMap::from([(
            "alu".into(),
            ExecutionUnitConfig {
                matrix_accumulator_capacity_bytes: None,
                capabilities: vec![cap],
            },
        )]),
    }
}

fn operation_class_capability(
    class: &str,
    inputs: Option<Vec<Vec<DataType>>>,
    rate: f64,
) -> CapabilityConfig {
    CapabilityConfig {
        kind: CapabilityKind::Scalar,
        class: class.into(),
        input_dtypes: inputs,
        ops_per_tick: rate,
    }
}

fn operation_class(operations: &[&str], arity: usize, flops_per_op: usize) -> OperationClass {
    OperationClass {
        operations: operations.iter().map(|name| (*name).into()).collect(),
        arity,
        flops_per_op,
    }
}

fn compute_model(
    classes: &BTreeMap<String, OperationClass>,
    execution_units: &[(&str, Vec<CapabilityConfig>)],
) -> ComputeModel {
    let config = ComputeConfig {
        execution_units: execution_units
            .iter()
            .map(|(name, caps)| {
                (
                    (*name).into(),
                    ExecutionUnitConfig {
                        matrix_accumulator_capacity_bytes: None,
                        capabilities: caps.clone(),
                    },
                )
            })
            .collect(),
    };
    ComputeModel::new(&config, classes).unwrap()
}

#[test]
fn compact_class_capabilities_round_trip() {
    let yaml = "execution_units: { vector: { capabilities: [{ class: math, input_dtypes: [[fp16, fp16], [bf16, bf16]], ops_per_tick: 200 }] } }";
    let config: ComputeConfig = from_str(yaml).unwrap();
    let classes = BTreeMap::from([("math".into(), operation_class(&["add", "mul"], 2, 1))]);
    ComputeModel::new(&config, &classes).unwrap();
    let emitted = to_string(&config).unwrap();
    assert!(emitted.contains("class: math"));
    assert_eq!(config, from_str(&emitted).unwrap());
}

#[test]
fn explicit_datatype_rate_overrides_class_fallback() {
    let classes = BTreeMap::from([("math".into(), operation_class(&["add", "mul"], 2, 1))]);
    let model = compute_model(
        &classes,
        &[(
            "alu",
            vec![
                operation_class_capability("math", None, 10.0),
                operation_class_capability(
                    "math",
                    Some(vec![vec![DataType::Fp16, DataType::Bf16]]),
                    100.0,
                ),
            ],
        )],
    );
    let work = ComputeWork {
        stages: vec![WorkStage {
            operations: vec![
                TypedWork::operation("add", vec![DataType::Fp16, DataType::Bf16], 100),
                TypedWork::operation("mul", vec![DataType::Bf16, DataType::Fp16], 100),
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
fn compute_plan_overlaps_service_costs_on_independent_execution_units() {
    let classes = BTreeMap::from([
        ("trig".into(), operation_class(&["sin", "cos"], 1, 1)),
        ("math".into(), operation_class(&["add"], 2, 1)),
    ]);
    let two_execution_units = compute_model(
        &classes,
        &[
            ("trig", vec![operation_class_capability("trig", None, 10.0)]),
            (
                "vector",
                vec![operation_class_capability("math", None, 10.0)],
            ),
        ],
    );
    let plan = two_execution_units
        .lower(&ComputeWork::parallel(vec![
            ComputeWork::operation("sin", vec![DataType::Fp32], 10),
            ComputeWork::operation("add", vec![DataType::Fp32; 2], 10),
        ]))
        .unwrap();
    assert_eq!(plan.service_ticks().unwrap(), 1);
    assert_eq!(plan.stages[0].len(), 2);
}

#[test]
fn parallel_composition_preserves_sequential_stages() {
    let model = compute_model(
        &BTreeMap::from([
            ("trig".into(), operation_class(&["sin"], 1, 1)),
            ("math".into(), operation_class(&["add"], 2, 1)),
        ]),
        &[
            ("trig", vec![operation_class_capability("trig", None, 10.0)]),
            (
                "vector",
                vec![operation_class_capability("math", None, 10.0)],
            ),
        ],
    );
    let mut sequential = ComputeWork::operation("sin", vec![DataType::Fp32], 10);
    sequential
        .stages
        .extend(ComputeWork::operation("add", vec![DataType::Fp32; 2], 10).stages);
    for works in [
        vec![sequential.clone()],
        vec![
            sequential,
            ComputeWork::operation("add", vec![DataType::Fp32; 2], 10),
        ],
    ] {
        let plan = model.lower(&ComputeWork::parallel(works)).unwrap();
        assert_eq!(plan.stages.len(), 2);
        assert_eq!(plan.service_ticks().unwrap(), 2);
        assert_eq!(plan.stages[1][0].machine_ops.by_operation["add"], 10);
    }
}

#[test]
fn equivalent_execution_units_are_available_to_the_same_operation() {
    let classes = BTreeMap::from([("trig".into(), operation_class(&["sin", "cos"], 1, 1))]);
    let ambiguous = compute_model(
        &classes,
        &[
            ("a", vec![operation_class_capability("trig", None, 10.0)]),
            ("b", vec![operation_class_capability("trig", None, 20.0)]),
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
fn declared_predicate_operation_accepts_its_input_arity() {
    assert!(
        predicate_model()
            .validate_work(&ComputeWork::operation("is_nan", vec![DataType::Fp32], 4))
            .is_ok()
    );
}

#[test]
fn declared_predicate_operation_rejects_wrong_input_arity() {
    let error = predicate_model()
        .validate_work(&ComputeWork::operation(
            "is_nan",
            vec![DataType::Fp32; 2],
            4,
        ))
        .unwrap_err()
        .to_string();
    assert!(error.contains("unsupported capability"), "{error}");
}

#[test]
fn model_rejects_an_undeclared_operation() {
    let error = predicate_model()
        .validate_work(&ComputeWork::operation("sin", vec![DataType::Fp32], 4))
        .unwrap_err()
        .to_string();
    assert!(error.contains("unsupported capability"), "{error}");
}

#[test]
fn capability_rejects_an_unknown_class() {
    assert!(
        ComputeModel::new(
            &single_capability_config(operation_class_capability("missing", None, 1.0)),
            &BTreeMap::new()
        )
        .is_err()
    );
}

#[test]
fn capability_rejects_invalid_throughput() {
    for rate in [0.0, f64::NAN] {
        assert!(
            ComputeModel::new(
                &single_capability_config(operation_class_capability("trig", None, rate)),
                &BTreeMap::from([("trig".into(), operation_class(&["sin"], 1, 1))])
            )
            .is_err()
        );
    }
}

#[test]
fn capability_rejects_dtype_signature_arity_mismatch() {
    assert!(
        ComputeModel::new(
            &single_capability_config(operation_class_capability(
                "trig",
                Some(vec![vec![DataType::Fp32; 2]]),
                1.0
            )),
            &BTreeMap::from([("trig".into(), operation_class(&["sin"], 1, 1))])
        )
        .is_err()
    );
}

#[test]
fn capability_rejects_an_empty_dtype_signature_list() {
    assert!(
        ComputeModel::new(
            &single_capability_config(operation_class_capability("trig", Some(vec![]), 1.0)),
            &BTreeMap::from([("trig".into(), operation_class(&["sin"], 1, 1))])
        )
        .is_err()
    );
}

#[test]
fn class_rejects_duplicate_operations() {
    assert!(
        ComputeModel::new(
            &single_capability_config(operation_class_capability("trig", None, 1.0)),
            &BTreeMap::from([("trig".into(), operation_class(&["sin", "sin"], 1, 1))])
        )
        .is_err()
    );
}

#[test]
fn class_rejects_uppercase_operations() {
    let error = ComputeModel::new(
        &single_capability_config(operation_class_capability("trig", None, 1.0)),
        &BTreeMap::from([("trig".into(), operation_class(&["Sin"], 1, 1))]),
    )
    .unwrap_err();
    assert!(error.to_string().contains("lowercase"), "{error}");
}

#[test]
fn zero_count_scalar_work_requires_no_capability_or_service() {
    let model = predicate_model();
    for (op, inputs) in [("is_nan", vec![DataType::Fp32]), ("undeclared", vec![])] {
        let work = ComputeWork::operation(op, inputs, 0);
        model.validate_work(&work).unwrap();
        let plan = model.lower(&work).unwrap();
        assert!(plan.stages.iter().flatten().next().is_none());
        assert_eq!(plan.service_ticks().unwrap(), 0);
    }
}

#[test]
fn unrepresentable_candidate_duration_does_not_poison_serviceable_units() {
    for (name, units, copies, expected) in [
        ("slow_only", vec![("slow", 1e-19)], 1, None),
        ("fast_only", vec![("fast", 1.0)], 1, Some((2, 2))),
        (
            "fast_and_slow",
            vec![("fast", 1.0), ("slow", 1e-19)],
            1,
            Some((2, 2)),
        ),
        (
            "all_slow",
            vec![("slow", 1e-19), ("slower", 8e-20)],
            1,
            None,
        ),
        // Three 2-op items at rate 10 coalesce to ceil(0.6) = 1 tick.
        ("fast_batched", vec![("fast", 10.0)], 3, Some((1, 6))),
        (
            "fast_batched_with_slow",
            vec![("fast", 10.0), ("slow", 1e-19)],
            3,
            Some((1, 6)),
        ),
    ] {
        let classes = BTreeMap::from([("exp".into(), operation_class(&["exp"], 1, 1))]);
        let units = units
            .into_iter()
            .map(|(name, rate)| (name, vec![operation_class_capability("exp", None, rate)]))
            .collect::<Vec<_>>();
        let model = compute_model(&classes, &units);
        let result = model.lower(&ComputeWork::parallel(vec![
            ComputeWork::operation(
                "exp",
                vec![DataType::Fp32],
                2
            );
            copies
        ]));
        if let Some((ticks, ops)) = expected {
            let plan = result.unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(plan.service_ticks().unwrap(), ticks, "{name}");
            assert_eq!(plan.stages[0].len(), 1, "{name}");
            let batch = &plan.stages[0][0];
            assert_eq!(batch.alternatives.len(), 1, "{name}");
            assert_eq!(batch.alternatives[0].1, ticks, "{name}");
            assert_eq!(
                model.execution_unit_names()[batch.alternatives[0].0.0],
                "fast",
                "{name}"
            );
            assert_eq!(batch.machine_ops.by_operation["exp"], ops, "{name}");
            assert_eq!(batch.machine_ops.flop_equivalents(), ops, "{name}");
        } else {
            assert!(
                result.unwrap_err().to_string().contains("duration"),
                "{name}"
            );
        }
    }
}
