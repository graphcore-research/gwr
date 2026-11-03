// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
// Pure candidate service-shape controls; runtime geometry has separate native
// gates.

use std::collections::BTreeMap;

use yaml_serde::{from_str, from_value, to_value};

use crate::processing_element::MachineOpCounts;
use crate::processing_element::compute::{
    CapabilityConfig, CapabilityKind, ComputeConfig, ComputeModel, ComputeWork, ExecutionPlan,
    ExecutionUnitConfig, ExecutionUnitId, MatrixServiceTile, MatrixWorkShape, OperationClass,
    TypedWork,
};
use crate::processing_element::tensors::dtype::DataType;

const MATRIX_MACS_PER_TICK: usize = 256;
const FLOPS_PER_MAC: usize = 2;
const M: usize = 16;
const N: usize = 16;
const K: usize = 16;

// The square fills one service block. The skinny output has the same number
// of elements, but only one of the block's rows performs useful work.
const SKINNY_COLUMNS: usize = M * N;
const LOGICAL_MACS: usize = M * N * K;
const SKINNY_SERVICE_MACS: usize = M * SKINNY_COLUMNS * K;
const SQUARE_SERVICE_TICKS: u64 = LOGICAL_MACS.div_ceil(MATRIX_MACS_PER_TICK) as u64;
const SKINNY_SERVICE_TICKS: u64 = SKINNY_SERVICE_MACS.div_ceil(MATRIX_MACS_PER_TICK) as u64;

fn matrix_service_tile(m: usize, n: usize, k: usize) -> MatrixServiceTile {
    MatrixServiceTile { m, n, k }
}

fn matrix_capability(policy: Option<MatrixServiceTile>) -> CapabilityConfig {
    CapabilityConfig {
        class: "matrix".into(),
        input_dtypes: Some(vec![vec![DataType::Bf16; 2]]),
        ops_per_tick: MATRIX_MACS_PER_TICK as f64,
        kind: CapabilityKind::Matrix {
            accumulator: DataType::Fp32,
            matrix_service_tile: policy,
        },
    }
}

fn matrix_compute_model(entries: Vec<(&str, CapabilityConfig)>) -> ComputeModel {
    ComputeModel::new(&compute_config(entries), &matrix_operation_classes()).unwrap()
}

fn matrix_operation_classes() -> BTreeMap<String, OperationClass> {
    BTreeMap::from([(
        "matrix".into(),
        OperationClass {
            operations: vec!["matrix_mac".into()],
            arity: 2,
            flops_per_op: FLOPS_PER_MAC,
        },
    )])
}

fn compute_config(entries: Vec<(&str, CapabilityConfig)>) -> ComputeConfig {
    ComputeConfig {
        execution_units: entries
            .into_iter()
            .map(|(name, capability)| {
                (
                    name.into(),
                    ExecutionUnitConfig {
                        matrix_accumulator_capacity_bytes: None,
                        capabilities: vec![capability],
                    },
                )
            })
            .collect(),
    }
}

fn matrix_work(m: usize, n: usize, k: usize) -> ComputeWork {
    ComputeWork::matrix_mac(
        vec![DataType::Bf16; 2],
        DataType::Fp32,
        matrix_work_shape(m, n, k),
    )
    .unwrap()
}

fn matrix_work_shape(m: usize, n: usize, k: usize) -> MatrixWorkShape {
    MatrixWorkShape {
        batches: 1,
        m,
        n,
        k,
    }
}

fn machine_operation_counts(plan: &ExecutionPlan) -> MachineOpCounts {
    plan.stages
        .iter()
        .flatten()
        .fold(MachineOpCounts::default(), |sum, batch| {
            sum.checked_add(batch.machine_ops.clone()).unwrap()
        })
}

#[test]
fn square_and_skinny_keep_equal_logical_macs_but_charge_different_service() {
    let model = matrix_compute_model(vec![(
        "matrix",
        matrix_capability(Some(matrix_service_tile(M, N, K))),
    )]);
    let square = model.lower(&matrix_work(M, N, K)).unwrap();
    let skinny = model.lower(&matrix_work(1, SKINNY_COLUMNS, K)).unwrap();
    assert_eq!(square.service_ticks().unwrap(), SQUARE_SERVICE_TICKS);
    assert_eq!(skinny.service_ticks().unwrap(), SKINNY_SERVICE_TICKS);
    assert_eq!(
        machine_operation_counts(&square),
        machine_operation_counts(&skinny)
    );
    assert_eq!(
        machine_operation_counts(&skinny).by_operation["matrix_mac"],
        LOGICAL_MACS
    );
    assert_eq!(
        machine_operation_counts(&skinny).flop_equivalents,
        LOGICAL_MACS * FLOPS_PER_MAC
    );
}

#[test]
fn each_candidate_uses_its_own_service_tile_and_preserves_logical_counts() {
    let model = matrix_compute_model(vec![
        (
            "a_macro16",
            matrix_capability(Some(matrix_service_tile(M, N, K))),
        ),
        (
            "b_macro1",
            matrix_capability(Some(matrix_service_tile(1, 1, 1))),
        ),
    ]);
    let plan = model.lower(&matrix_work(1, SKINNY_COLUMNS, K)).unwrap();
    assert_eq!(
        plan.stages[0][0].alternatives,
        vec![
            (ExecutionUnitId(1), SQUARE_SERVICE_TICKS),
            (ExecutionUnitId(0), SKINNY_SERVICE_TICKS)
        ]
    );
    assert_eq!(plan.service_ticks().unwrap(), SQUARE_SERVICE_TICKS);
    assert_eq!(
        machine_operation_counts(&plan).by_operation["matrix_mac"],
        LOGICAL_MACS
    );
    // Check each candidate separately: choosing the faster execution unit above
    // does not verify the slower execution unit's service time or operation
    // counts.
    for (policy, ticks) in [
        (matrix_service_tile(M, N, K), SKINNY_SERVICE_TICKS),
        (matrix_service_tile(1, 1, 1), SQUARE_SERVICE_TICKS),
    ] {
        let single = matrix_compute_model(vec![("only", matrix_capability(Some(policy)))])
            .lower(&matrix_work(1, SKINNY_COLUMNS, K))
            .unwrap();
        assert_eq!(single.service_ticks().unwrap(), ticks);
        assert_eq!(
            machine_operation_counts(&single),
            machine_operation_counts(&plan)
        );
    }
}

#[test]
fn plain_and_shaped_candidates_keep_independent_service_costs() {
    let model = matrix_compute_model(vec![
        ("a_plain", matrix_capability(None)),
        (
            "b_shaped",
            matrix_capability(Some(matrix_service_tile(M, N, K))),
        ),
    ]);
    let plan = model.lower(&matrix_work(1, SKINNY_COLUMNS, K)).unwrap();
    assert_eq!(
        plan.stages[0][0].alternatives,
        vec![
            (ExecutionUnitId(0), SQUARE_SERVICE_TICKS),
            (ExecutionUnitId(1), SKINNY_SERVICE_TICKS)
        ]
    );
    assert_eq!(
        machine_operation_counts(&plan).by_operation["matrix_mac"],
        LOGICAL_MACS
    );
}

#[test]
fn matrix_work_on_the_same_execution_unit_adds_service_costs() {
    let one = matrix_compute_model(vec![(
        "matrix",
        matrix_capability(Some(matrix_service_tile(M, N, K))),
    )]);
    let sum = one
        .lower(&ComputeWork::parallel(vec![
            matrix_work(M, N, K),
            matrix_work(1, SKINNY_COLUMNS, K),
        ]))
        .unwrap();
    assert_eq!(sum.stages[0].len(), 1);
    assert_eq!(
        sum.service_ticks().unwrap(),
        (LOGICAL_MACS + SKINNY_SERVICE_MACS).div_ceil(MATRIX_MACS_PER_TICK) as u64
    );
    assert_eq!(
        machine_operation_counts(&sum).by_operation["matrix_mac"],
        2 * LOGICAL_MACS
    );
}

#[test]
fn compute_plan_overlaps_vector_and_matrix_service_costs() {
    const VECTOR_OPS_PER_TICK: usize = 100;
    const VECTOR_SERVICE_TICKS: u64 = 256;
    const VECTOR_OPERATIONS: usize = VECTOR_OPS_PER_TICK * VECTOR_SERVICE_TICKS as usize;
    let mut two_classes = matrix_operation_classes();
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
        ops_per_tick: VECTOR_OPS_PER_TICK as f64,
        kind: CapabilityKind::Scalar,
    };
    let two = ComputeModel::new(
        &compute_config(vec![
            (
                "matrix",
                matrix_capability(Some(matrix_service_tile(M, N, K))),
            ),
            ("vector", vector),
        ]),
        &two_classes,
    )
    .unwrap();
    let overlap = two
        .lower(&ComputeWork::parallel(vec![
            matrix_work(M, N, K),
            ComputeWork::operation("add", vec![DataType::Fp32; 2], VECTOR_OPERATIONS),
        ]))
        .unwrap();
    assert_eq!(overlap.stages[0].len(), 2);
    assert_eq!(
        overlap.service_ticks().unwrap(),
        VECTOR_SERVICE_TICKS.max(SQUARE_SERVICE_TICKS)
    );
    assert_eq!(
        machine_operation_counts(&overlap).by_operation["matrix_mac"],
        LOGICAL_MACS
    );
    assert_eq!(
        machine_operation_counts(&overlap).by_operation["add"],
        VECTOR_OPERATIONS
    );
}

#[test]
fn matrix_work_rejects_zero_dimensions() {
    for dims in [
        matrix_work_shape(0, 1, 1),
        matrix_work_shape(1, 0, 1),
        matrix_work_shape(1, 1, 0),
        MatrixWorkShape {
            batches: 0,
            ..matrix_work_shape(1, 1, 1)
        },
    ] {
        assert!(dims.logical_macs().is_err());
        assert!(ComputeWork::matrix_mac(vec![DataType::Bf16; 2], DataType::Fp32, dims).is_err());
    }
}

#[test]
fn matrix_work_rejects_logical_mac_count_overflow() {
    let dims = matrix_work_shape(usize::MAX, 2, 1);
    assert!(dims.logical_macs().is_err());
    assert!(ComputeWork::matrix_mac(vec![DataType::Bf16; 2], DataType::Fp32, dims).is_err());
}

#[test]
fn matrix_work_requires_two_operands() {
    assert!(
        ComputeWork::matrix_mac(
            vec![DataType::Bf16],
            DataType::Fp32,
            matrix_work_shape(1, 1, 1)
        )
        .is_err()
    );
}

#[test]
fn service_shape_rejects_mismatched_logical_work_even_when_count_is_zero() {
    let model = matrix_compute_model(vec![(
        "matrix",
        matrix_capability(Some(matrix_service_tile(M, N, K))),
    )]);
    let mut mismatch = matrix_work(M, N, K);
    mismatch.stages[0].operations[0].count -= 1;
    assert!(model.lower(&mismatch).is_err());
    mismatch.stages[0].operations[0].count = 0;
    assert!(
        model.lower(&mismatch).is_err(),
        "zero count must not skip malformed shape validation"
    );
}

#[test]
fn service_shape_rejects_padded_mac_count_overflow() {
    // Macro itself fits usize; rounding a 2x2 matrix overflows service count
    // only.
    let overflow = matrix_compute_model(vec![(
        "matrix",
        matrix_capability(Some(matrix_service_tile(usize::MAX, 1, 1))),
    )]);
    assert!(overflow.lower(&matrix_work(2, 2, 1)).is_err());
}

#[test]
fn service_tile_policy_requires_explicit_operand_dtypes() {
    let mut invalid = matrix_capability(Some(matrix_service_tile(M, N, K)));
    invalid.input_dtypes = None;
    assert!(
        ComputeModel::new(
            &compute_config(vec![("matrix", invalid)]),
            &matrix_operation_classes()
        )
        .is_err()
    );
}

#[test]
fn service_tile_policy_rejects_a_zero_dimension() {
    assert!(
        ComputeModel::new(
            &compute_config(vec![(
                "matrix",
                matrix_capability(Some(matrix_service_tile(0, N, K)))
            )]),
            &matrix_operation_classes()
        )
        .is_err()
    );
}

#[test]
fn service_tile_policy_rejects_dimension_product_overflow() {
    assert!(
        ComputeModel::new(
            &compute_config(vec![(
                "matrix",
                matrix_capability(Some(matrix_service_tile(usize::MAX, 2, 1)))
            )]),
            &matrix_operation_classes()
        )
        .is_err()
    );
}

#[test]
fn service_tile_policy_rejects_a_class_with_scalar_operations() {
    let mut mixed = matrix_operation_classes();
    mixed
        .get_mut("matrix")
        .unwrap()
        .operations
        .push("add".into());
    assert!(
        ComputeModel::new(
            &compute_config(vec![(
                "matrix",
                matrix_capability(Some(matrix_service_tile(M, N, K)))
            )]),
            &mixed
        )
        .is_err()
    );
}

#[test]
fn service_tile_policy_requires_two_input_class_arity() {
    let mut arity = matrix_operation_classes();
    arity.get_mut("matrix").unwrap().arity = 1;
    assert!(
        ComputeModel::new(
            &compute_config(vec![(
                "matrix",
                matrix_capability(Some(matrix_service_tile(M, N, K)))
            )]),
            &arity
        )
        .is_err()
    );
}

#[test]
fn service_tile_dimensions_reject_noninteger_yaml_values() {
    for bad in [r#"{"m":16.0,"n":16,"k":16}"#, r#"{"m":true,"n":16,"k":16}"#] {
        assert!(from_str::<MatrixServiceTile>(bad).is_err());
    }
}

#[test]
fn service_tile_policy_rejects_unknown_yaml_fields() {
    assert!(from_str::<MatrixServiceTile>(r#"{"m":16,"n":16,"k":16,"unknown":true}"#).is_err());
}

#[test]
fn omitted_service_tile_stays_omitted_after_yaml_round_trip() {
    let policyless = matrix_capability(None);
    let encoded = to_value(&policyless).unwrap();
    assert!(
        !encoded
            .as_mapping()
            .unwrap()
            .contains_key("matrix_service_tile")
    );
    let roundtrip: CapabilityConfig = from_value(encoded.clone()).unwrap();
    assert_eq!(roundtrip, policyless);
    assert_eq!(to_value(roundtrip).unwrap(), encoded);
}

#[test]
fn scalar_work_round_trips_without_matrix_geometry() {
    let scalar_work = ComputeWork::operation("add", vec![DataType::Fp32; 2], 4096);
    let demand = &scalar_work.stages[0].operations[0];
    let encoded_demand = to_value(demand).unwrap();
    assert!(
        !encoded_demand
            .as_mapping()
            .unwrap()
            .contains_key("matrix_shape")
    );
    let decoded: TypedWork = from_value(encoded_demand.clone()).unwrap();
    assert_eq!(to_value(decoded).unwrap(), encoded_demand);
}

#[test]
fn matrix_work_without_service_tiles_uses_logical_mac_count_for_timing() {
    let model = matrix_compute_model(vec![("matrix", matrix_capability(None))]);
    let shape = matrix_work_shape(1, SKINNY_COLUMNS, K);
    let plan = model
        .lower(&matrix_work(shape.m, shape.n, shape.k))
        .unwrap();
    let logical_macs = shape.logical_macs().unwrap();
    assert_eq!(
        plan.service_ticks().unwrap(),
        logical_macs.div_ceil(MATRIX_MACS_PER_TICK) as u64
    );
    assert_eq!(
        machine_operation_counts(&plan).by_operation["matrix_mac"],
        logical_macs
    );
}

#[test]
fn batched_matrix_service_rounds_each_matrix_before_multiplying_by_batch_count() {
    const BATCHES: usize = 3;
    let shape = MatrixWorkShape {
        batches: BATCHES,
        m: 1,
        n: 5,
        k: 7,
    };
    let work = ComputeWork::matrix_mac(vec![DataType::Bf16; 2], DataType::Fp32, shape).unwrap();
    let model = matrix_compute_model(vec![(
        "matrix",
        matrix_capability(Some(matrix_service_tile(M, N, K))),
    )]);
    let plan = model.lower(&work).unwrap();
    let service_macs = BATCHES * M * N * K;
    assert_eq!(
        plan.service_ticks().unwrap(),
        service_macs.div_ceil(MATRIX_MACS_PER_TICK) as u64
    );
    assert_eq!(
        machine_operation_counts(&plan).by_operation["matrix_mac"],
        shape.logical_macs().unwrap()
    );
}

#[test]
fn matrix_batch_count_overflow_is_rejected() {
    let shape = MatrixWorkShape {
        batches: usize::MAX,
        ..matrix_work_shape(2, 1, 1)
    };
    assert!(ComputeWork::matrix_mac(vec![DataType::Bf16; 2], DataType::Fp32, shape).is_err());
}
