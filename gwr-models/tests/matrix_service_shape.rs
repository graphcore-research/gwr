// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
// Pure candidate service-shape controls; runtime geometry has separate native
// gates.

use std::collections::BTreeMap;

use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::compute::{
    CapabilityConfig, ComputeConfig, ComputeModel, ComputeWork, EngineConfig, EngineId,
    ExecutionMode, MatrixServiceTile, MatrixWorkShape, OperationClass, Sparsity, TypedWork,
};
use gwr_models::processing_element::tensors::dtype::DataType;

fn tile(m: usize, n: usize, k: usize) -> MatrixServiceTile {
    MatrixServiceTile { m, n, k }
}
fn shape(m: usize, n: usize, k: usize) -> MatrixWorkShape {
    MatrixWorkShape { m, n, k }
}
fn mode() -> ExecutionMode {
    ExecutionMode {
        instruction: "tensor".into(),
        accumulator: DataType::Fp32,
        sparsity: Sparsity::Dense,
    }
}
fn cap(policy: Option<MatrixServiceTile>) -> CapabilityConfig {
    CapabilityConfig {
        class: "matrix".into(),
        input_dtypes: Some(vec![vec![DataType::Bf16; 2]]),
        ops_per_tick: 256.0,
        execution: Some(mode()),
        matrix_service_tile: policy,
    }
}
fn classes() -> BTreeMap<String, OperationClass> {
    BTreeMap::from([(
        "matrix".into(),
        OperationClass {
            operations: vec!["matrix_mac".into()],
            arity: 2,
            flops_per_op: 2,
        },
    )])
}
fn config(entries: Vec<(&str, CapabilityConfig)>) -> ComputeConfig {
    ComputeConfig {
        engines: entries
            .into_iter()
            .map(|(name, capability)| {
                (
                    name.into(),
                    EngineConfig {
                        matrix_accumulator_capacity_bytes: None,
                        capabilities: vec![capability],
                    },
                )
            })
            .collect(),
    }
}
fn model(entries: Vec<(&str, CapabilityConfig)>) -> ComputeModel {
    ComputeModel::new(&config(entries), &classes()).unwrap()
}
fn work(m: usize, n: usize, k: usize) -> ComputeWork {
    ComputeWork::matrix_mac(vec![DataType::Bf16; 2], shape(m, n, k))
        .unwrap()
        .with_execution(Some(mode()))
}
fn counts(plan: &gwr_models::processing_element::compute::ExecutionPlan) -> MachineOpCounts {
    plan.stages
        .iter()
        .flatten()
        .fold(MachineOpCounts::default(), |sum, batch| {
            sum.checked_add(batch.machine_ops.clone()).unwrap()
        })
}

#[test]
fn square_and_skinny_keep_equal_logical_macs_but_charge_different_service() {
    let model = model(vec![("matrix", cap(Some(tile(16, 16, 16))))]);
    let square = model.lower(&work(16, 16, 16)).unwrap();
    let skinny = model.lower(&work(1, 256, 16)).unwrap();
    assert_eq!(square.service_ticks().unwrap(), 16);
    assert_eq!(skinny.service_ticks().unwrap(), 256);
    assert_eq!(counts(&square), counts(&skinny));
    assert_eq!(counts(&skinny).by_operation["matrix_mac"], 4096);
    assert_eq!(counts(&skinny).flop_equivalents, 8192);
}

#[test]
fn each_candidate_uses_its_own_macro_without_changing_logical_counts() {
    let model = model(vec![
        ("a_macro16", cap(Some(tile(16, 16, 16)))),
        ("b_macro1", cap(Some(tile(1, 1, 1)))),
    ]);
    let plan = model.lower(&work(1, 256, 16)).unwrap();
    assert_eq!(
        plan.stages[0][0].alternatives,
        vec![(EngineId(1), 16), (EngineId(0), 256)]
    );
    assert_eq!(plan.service_ticks().unwrap(), 16);
    assert_eq!(counts(&plan).by_operation["matrix_mac"], 4096);
    // Independently exercise each choice; selecting b above must not conceal
    // drift in a.
    for (policy, ticks) in [(tile(16, 16, 16), 256), (tile(1, 1, 1), 16)] {
        let single = self::model(vec![("only", cap(Some(policy)))])
            .lower(&work(1, 256, 16))
            .unwrap();
        assert_eq!(single.service_ticks().unwrap(), ticks);
        assert_eq!(counts(&single), counts(&plan));
    }
}

#[test]
fn plain_candidate_is_independent_but_does_not_bypass_missing_shape_guard() {
    let model = model(vec![
        ("a_plain", cap(None)),
        ("b_shaped", cap(Some(tile(16, 16, 16)))),
    ]);
    let plan = model.lower(&work(1, 256, 16)).unwrap();
    assert_eq!(
        plan.stages[0][0].alternatives,
        vec![(EngineId(0), 16), (EngineId(1), 256)]
    );
    assert_eq!(counts(&plan).by_operation["matrix_mac"], 4096);
    let missing = ComputeWork::operation("matrix_mac", vec![DataType::Bf16; 2], 4096)
        .with_execution(Some(mode()));
    assert!(model.lower(&missing).is_err());
}

#[test]
fn same_engine_service_adds_while_an_independent_vector_engine_overlaps() {
    let one = model(vec![("matrix", cap(Some(tile(16, 16, 16))))]);
    let sum = one
        .lower(&ComputeWork::parallel(vec![
            work(16, 16, 16),
            work(1, 256, 16),
        ]))
        .unwrap();
    assert_eq!(sum.stages[0].len(), 1);
    assert_eq!(sum.service_ticks().unwrap(), 272);
    assert_eq!(counts(&sum).by_operation["matrix_mac"], 8192);
    let mut two_classes = classes();
    two_classes.insert(
        "add".into(),
        OperationClass {
            operations: vec!["add".into()],
            arity: 2,
            flops_per_op: 1,
        },
    );
    let vector = CapabilityConfig {
        class: "add".into(),
        input_dtypes: Some(vec![vec![DataType::Fp32; 2]]),
        ops_per_tick: 100.0,
        execution: None,
        matrix_service_tile: None,
    };
    let two = ComputeModel::new(
        &config(vec![
            ("matrix", cap(Some(tile(16, 16, 16)))),
            ("vector", vector),
        ]),
        &two_classes,
    )
    .unwrap();
    let overlap = two
        .lower(&ComputeWork::parallel(vec![
            work(16, 16, 16),
            ComputeWork::operation("add", vec![DataType::Fp32; 2], 25600),
        ]))
        .unwrap();
    assert_eq!(overlap.stages[0].len(), 2);
    assert_eq!(overlap.service_ticks().unwrap(), 256);
    assert_eq!(counts(&overlap).by_operation["matrix_mac"], 4096);
    assert_eq!(counts(&overlap).by_operation["add"], 25600);
}

#[test]
fn malformed_shapes_and_checked_service_overflow_reject() {
    for dims in [
        shape(0, 1, 1),
        shape(1, 0, 1),
        shape(1, 1, 0),
        shape(usize::MAX, 2, 1),
    ] {
        assert!(dims.logical_macs().is_err());
        assert!(ComputeWork::matrix_mac(vec![DataType::Bf16; 2], dims).is_err());
    }
    assert!(ComputeWork::matrix_mac(vec![DataType::Bf16], shape(1, 1, 1)).is_err());
    let model = model(vec![("matrix", cap(Some(tile(16, 16, 16))))]);
    let mut mismatch = work(16, 16, 16);
    mismatch.stages[0].operations[0].count -= 1;
    assert!(model.lower(&mismatch).is_err());
    mismatch.stages[0].operations[0].count = 0;
    assert!(
        model.lower(&mismatch).is_err(),
        "zero count must not skip malformed shape validation"
    );
    // Macro itself fits usize; rounding a 2x2 matrix overflows service count
    // only.
    let overflow = self::model(vec![("matrix", cap(Some(tile(usize::MAX, 1, 1))))]);
    assert!(overflow.lower(&work(2, 2, 1)).is_err());
}

#[test]
fn policy_declaration_is_strict_and_only_exact_dense_matrix() {
    let mut cases = Vec::new();
    let mut invalid = cap(Some(tile(16, 16, 16)));
    invalid.input_dtypes = None;
    cases.push((invalid, classes()));
    let mut invalid = cap(Some(tile(16, 16, 16)));
    invalid.execution = None;
    cases.push((invalid, classes()));
    let mut invalid = cap(Some(tile(16, 16, 16)));
    invalid.execution.as_mut().unwrap().sparsity = Sparsity::Structured;
    cases.push((invalid, classes()));
    cases.push((cap(Some(tile(0, 16, 16))), classes()));
    cases.push((cap(Some(tile(usize::MAX, 2, 1))), classes()));
    let mut mixed = classes();
    mixed
        .get_mut("matrix")
        .unwrap()
        .operations
        .push("add".into());
    cases.push((cap(Some(tile(16, 16, 16))), mixed));
    let mut arity = classes();
    arity.get_mut("matrix").unwrap().arity = 1;
    cases.push((cap(Some(tile(16, 16, 16))), arity));
    for (invalid, classes) in cases {
        assert!(ComputeModel::new(&config(vec![("matrix", invalid)]), &classes).is_err());
    }
    for bad in [
        r#"{"m":16.0,"n":16,"k":16}"#,
        r#"{"m":true,"n":16,"k":16}"#,
        r#"{"m":16,"n":16,"k":16,"physical_padding":true}"#,
    ] {
        assert!(yaml_serde::from_str::<MatrixServiceTile>(bad).is_err());
    }
}

#[test]
fn none_preserves_legacy_work_config_bytes_and_service() {
    let policyless = cap(None);
    let encoded = yaml_serde::to_value(&policyless).unwrap();
    assert!(
        !encoded
            .as_mapping()
            .unwrap()
            .contains_key("matrix_service_tile")
    );
    let roundtrip: CapabilityConfig = yaml_serde::from_value(encoded.clone()).unwrap();
    assert_eq!(roundtrip, policyless);
    assert_eq!(yaml_serde::to_value(roundtrip).unwrap(), encoded);
    let legacy = ComputeWork::operation("matrix_mac", vec![DataType::Bf16; 2], 4096)
        .with_execution(Some(mode()));
    let demand = &legacy.stages[0].operations[0];
    let encoded_demand = yaml_serde::to_value(demand).unwrap();
    assert!(
        !encoded_demand
            .as_mapping()
            .unwrap()
            .contains_key("matrix_shape")
    );
    let decoded: TypedWork = yaml_serde::from_value(encoded_demand.clone()).unwrap();
    assert_eq!(yaml_serde::to_value(decoded).unwrap(), encoded_demand);
    let model = model(vec![("matrix", policyless)]);
    let old = model.lower(&legacy).unwrap();
    let shaped = model.lower(&work(1, 256, 16)).unwrap();
    assert_eq!(old.service_ticks().unwrap(), 16);
    assert_eq!(
        old.service_ticks().unwrap(),
        shaped.service_ticks().unwrap()
    );
    assert_eq!(counts(&old), counts(&shaped));
}

#[test]
fn physical_counters_keep_candidate_padding_separate_from_logical_macs() {
    let model = model(vec![
        ("a_macro16", cap(Some(tile(16, 16, 16)))),
        ("b_macro1", cap(Some(tile(1, 1, 1)))),
    ]);
    let plan = model.lower(&work(1, 256, 16)).unwrap();
    let batch = &plan.stages[0][0];
    let padded = &batch.actions[&EngineId(0)][0];
    let exact = &batch.actions[&EngineId(1)][0];
    assert_eq!(padded.logical_operations, 4096);
    assert_eq!(padded.service_operations, 65536);
    assert_eq!(exact.service_operations, 4096);
    let mut counters = gwr_models::processing_element::compute::ComputeCounters::default();
    counters.record_batch(batch, EngineId(0)).unwrap();
    let snapshot = counters.snapshot(model.engine_names());
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].engine, "a_macro16");
    assert_eq!(snapshot[0].service_operations, 65536);
}
