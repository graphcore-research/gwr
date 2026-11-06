// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
// Pure execution unit accumulator admission; runner residency is a separate
// gate.
use std::collections::BTreeMap;

use yaml_serde::{from_str, to_string};

use crate::processing_element::compute::{
    CapabilityConfig, CapabilityKind, ComputeConfig, ComputeModel, ComputeWork,
    ExecutionUnitConfig, ExecutionUnitId, MatrixServiceTile, MatrixWorkShape, OperationClass,
};
use crate::processing_element::tensors::dtype::DataType;

fn matrix_execution_unit_config(
    capacity: Option<u64>,
    tile: Option<MatrixServiceTile>,
) -> ExecutionUnitConfig {
    ExecutionUnitConfig {
        matrix_accumulator_capacity_bytes: capacity,
        capabilities: vec![matrix_capability(tile)],
    }
}

fn matrix_capability(tile: Option<MatrixServiceTile>) -> CapabilityConfig {
    CapabilityConfig {
        class: "matrix".into(),
        input_dtypes: Some(vec![vec![DataType::Bf16; 2]]),
        ops_per_tick: 256.0,
        kind: CapabilityKind::Matrix {
            accumulator: DataType::Fp32,
            matrix_service_tile: tile,
        },
    }
}

fn matrix_compute_model(entries: Vec<(&str, ExecutionUnitConfig)>) -> ComputeModel {
    ComputeModel::new(&compute_config(entries), &matrix_operation_classes()).unwrap()
}

fn matrix_operation_classes() -> BTreeMap<String, OperationClass> {
    BTreeMap::from([(
        "matrix".into(),
        OperationClass {
            operations: vec!["matrix_mac".into()],
            arity: 2,
            flops_per_op: 2,
        },
    )])
}

fn compute_config(entries: Vec<(&str, ExecutionUnitConfig)>) -> ComputeConfig {
    ComputeConfig {
        execution_units: entries
            .into_iter()
            .map(|(name, execution_unit)| (name.into(), execution_unit))
            .collect(),
    }
}

fn matrix_work(m: usize, n: usize, k: usize) -> ComputeWork {
    matrix_work_with_accumulator(m, n, k, DataType::Fp32)
}

fn matrix_work_with_accumulator(
    m: usize,
    n: usize,
    k: usize,
    accumulator: DataType,
) -> ComputeWork {
    ComputeWork::matrix_mac(
        [DataType::Bf16; 2],
        accumulator,
        MatrixWorkShape {
            batches: 1,
            m,
            n,
            k,
        },
    )
    .unwrap()
}

fn assert_lowering_error(model: &ComputeModel, work: &ComputeWork, category: &str) {
    let message = model.lower(work).unwrap_err().to_string();
    assert!(message.contains(category), "{message}");
}

#[test]
fn omitted_accumulator_capacity_stays_omitted_after_yaml_round_trip() {
    let none = matrix_execution_unit_config(None, None);
    let yaml = to_string(&none).unwrap();
    assert!(!yaml.contains("matrix_accumulator_capacity_bytes"));
    assert_eq!(from_str::<ExecutionUnitConfig>(&yaml).unwrap(), none);
}

#[test]
fn explicit_accumulator_capacity_round_trips() {
    let explicit = matrix_execution_unit_config(Some(256), None);
    let yaml = to_string(&explicit).unwrap();
    assert_eq!(from_str::<ExecutionUnitConfig>(&yaml).unwrap(), explicit);
}

#[test]
fn accumulator_capacity_rejects_yaml_values_outside_unsigned_integer_range() {
    for value in ["true", "1.5", "-1", "18446744073709551616"] {
        let invalid = format!(
            "matrix_accumulator_capacity_bytes: {value}\n{}",
            to_string(&matrix_execution_unit_config(None, None)).unwrap()
        );
        assert!(
            from_str::<ExecutionUnitConfig>(&invalid).is_err(),
            "{value}"
        );
    }
}

#[test]
fn execution_unit_configuration_rejects_unknown_yaml_fields() {
    let yaml = to_string(&matrix_execution_unit_config(Some(256), None)).unwrap();
    assert!(from_str::<ExecutionUnitConfig>(&format!("{yaml}unknown: 1\n")).is_err());
}

#[test]
fn accumulator_bytes_scale_with_output_elements_and_ignore_reduction_extent() {
    let model = matrix_compute_model(vec![(
        "matrix",
        matrix_execution_unit_config(Some(512), None),
    )]);
    for (m, n, k, bytes) in [
        (8, 8, 17, 256),
        (8, 8, 4096, 256),
        (2, 32, 17, 256),
        (8, 16, 1, 512),
    ] {
        let plan = model.lower(&matrix_work(m, n, k)).unwrap();
        assert_eq!(plan.stages[0][0].matrix_accumulator_bytes, bytes);
        assert_eq!(
            plan.stages[0][0].machine_ops.by_operation["matrix_mac"],
            m * n * k
        );
    }
}

#[test]
fn accumulator_byte_demand_overflow_is_rejected() {
    let model = matrix_compute_model(vec![(
        "matrix",
        matrix_execution_unit_config(Some(512), None),
    )]);
    assert_lowering_error(
        &model,
        &matrix_work(usize::MAX / 8 + 1, 1, 1),
        "accumulator byte demand overflows",
    );
}

#[test]
fn too_small_candidate_does_not_suppress_fit_or_unbounded_candidate() {
    let model = matrix_compute_model(vec![
        ("a_small", matrix_execution_unit_config(Some(128), None)),
        ("b_fit", matrix_execution_unit_config(Some(256), None)),
        ("c_unbounded", matrix_execution_unit_config(None, None)),
    ]);
    let plan = model.lower(&matrix_work(8, 8, 8)).unwrap();
    assert_eq!(
        plan.stages[0][0].alternatives,
        vec![(ExecutionUnitId(1), 2), (ExecutionUnitId(2), 2)]
    );
    assert_eq!(plan.stages[0][0].matrix_accumulator_bytes, 256);
    assert_eq!(
        plan.stages[0][0].machine_ops.by_operation["matrix_mac"],
        512
    );
}

#[test]
fn accumulator_capacity_rejects_work_when_no_candidate_fits() {
    assert_lowering_error(
        &matrix_compute_model(vec![(
            "only",
            matrix_execution_unit_config(Some(128), None),
        )]),
        &matrix_work(8, 8, 8),
        "capacity cannot hold logical output tile",
    );
}

#[test]
fn accumulator_capacity_filtering_does_not_hide_inconsistent_flop_costs() {
    // Capacity filtering must not conceal inconsistent mathematical flop costs.
    let mut inconsistent = compute_config(vec![
        ("a_small", matrix_execution_unit_config(Some(128), None)),
        ("b_fit", matrix_execution_unit_config(Some(256), None)),
    ]);
    inconsistent
        .execution_units
        .get_mut("a_small")
        .unwrap()
        .capabilities[0]
        .class = "other".into();
    let mut declarations = matrix_operation_classes();
    let mut other = declarations["matrix"].clone();
    other.flops_per_op = 1;
    declarations.insert("other".into(), other);
    let inconsistent = ComputeModel::new(&inconsistent, &declarations).unwrap();
    assert_lowering_error(
        &inconsistent,
        &matrix_work(8, 8, 8),
        "inconsistent flop costs",
    );
}

#[test]
fn service_tile_candidates_preserve_logical_counts_and_accumulator_demand() {
    let padded = MatrixServiceTile {
        m: 16,
        n: 16,
        k: 16,
    };
    let model = matrix_compute_model(vec![
        (
            "a_padded",
            matrix_execution_unit_config(Some(256), Some(padded)),
        ),
        ("b_plain", matrix_execution_unit_config(Some(256), None)),
    ]);
    let plan = model.lower(&matrix_work(2, 32, 16)).unwrap();
    let batch = &plan.stages[0][0];
    assert_eq!(
        batch.alternatives,
        vec![(ExecutionUnitId(1), 4), (ExecutionUnitId(0), 32)]
    );
    assert_eq!(batch.matrix_accumulator_bytes, 256);
    assert_eq!(batch.machine_ops.by_operation["matrix_mac"], 1024);
    assert_eq!(batch.machine_ops.flop_equivalents, 2048);
    let unconstrained = matrix_compute_model(vec![(
        "unconstrained",
        matrix_execution_unit_config(None, None),
    )])
    .lower(&matrix_work(2, 32, 16))
    .unwrap();
    assert_eq!(unconstrained.stages[0][0].matrix_accumulator_bytes, 0);
    assert_eq!(batch.machine_ops, unconstrained.stages[0][0].machine_ops);
}

#[test]
fn one_fixed_execution_unit_uses_maximum_not_sum_of_sequential_demands() {
    let model = matrix_compute_model(vec![(
        "matrix",
        matrix_execution_unit_config(Some(512), None),
    )]);
    let plan = model
        .lower(&ComputeWork::parallel(vec![
            matrix_work(8, 8, 1),
            matrix_work(8, 16, 1),
        ]))
        .unwrap();
    assert_eq!(plan.stages[0].len(), 1);
    let batch = &plan.stages[0][0];
    assert_eq!(batch.matrix_accumulator_bytes, 512);
    assert_eq!(batch.machine_ops.by_operation["matrix_mac"], 192);
    assert_eq!(plan.service_ticks().unwrap(), 1);
}

#[test]
fn accumulator_capacity_rejects_an_unsupported_accumulator() {
    let model = matrix_compute_model(vec![(
        "matrix",
        matrix_execution_unit_config(Some(256), None),
    )]);
    assert_lowering_error(
        &model,
        &matrix_work_with_accumulator(8, 8, 1, DataType::Int32),
        "unsupported capability",
    );
}

#[test]
fn accumulator_capacity_rejects_a_zero_capacity_declaration() {
    let invalid = compute_config(vec![(
        "matrix",
        matrix_execution_unit_config(Some(0), None),
    )]);
    let message = ComputeModel::new(&invalid, &matrix_operation_classes())
        .unwrap_err()
        .to_string();
    assert!(message.contains("capacity must be positive"), "{message}");
}

#[test]
fn accumulator_capacity_requires_exact_operand_dtypes() {
    let mut invalid = compute_config(vec![(
        "matrix",
        matrix_execution_unit_config(Some(256), None),
    )]);
    invalid
        .execution_units
        .get_mut("matrix")
        .unwrap()
        .capabilities[0]
        .input_dtypes = None;
    let message = ComputeModel::new(&invalid, &matrix_operation_classes())
        .unwrap_err()
        .to_string();
    assert!(message.contains("requires exact matrix"), "{message}");
}

#[test]
fn accumulator_capacity_requires_execution_unit_capabilities() {
    let mut invalid = compute_config(vec![(
        "matrix",
        matrix_execution_unit_config(Some(256), None),
    )]);
    invalid
        .execution_units
        .get_mut("matrix")
        .unwrap()
        .capabilities
        .clear();
    let message = ComputeModel::new(&invalid, &matrix_operation_classes())
        .unwrap_err()
        .to_string();
    assert!(message.contains("name and capabilities"), "{message}");
}

#[test]
fn accumulator_capacity_rejects_a_scalar_class() {
    let config = compute_config(vec![(
        "matrix",
        matrix_execution_unit_config(Some(256), None),
    )]);
    let scalar_classes = BTreeMap::from([(
        "matrix".into(),
        OperationClass {
            operations: vec!["add".into()],
            arity: 2,
            flops_per_op: 1,
        },
    )]);
    let message = ComputeModel::new(&config, &scalar_classes)
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("requires a matrix declaration"),
        "{message}"
    );
}

#[test]
fn matrix_accumulator_demand_includes_every_batch_output() {
    const BATCHES: usize = 3;
    const ROWS: usize = 2;
    const COLUMNS: usize = 4;
    const FP32_BYTES: usize = 4;
    const ACCUMULATOR_BYTES: usize = BATCHES * ROWS * COLUMNS * FP32_BYTES;
    let work = ComputeWork::matrix_mac(
        [DataType::Bf16; 2],
        DataType::Fp32,
        MatrixWorkShape {
            batches: BATCHES,
            m: ROWS,
            n: COLUMNS,
            k: 8,
        },
    )
    .unwrap();
    let model = matrix_compute_model(vec![(
        "matrix",
        matrix_execution_unit_config(Some(ACCUMULATOR_BYTES as u64), None),
    )]);
    let plan = model.lower(&work).unwrap();
    assert_eq!(
        plan.stages[0][0].matrix_accumulator_bytes,
        ACCUMULATOR_BYTES as u64
    );
    let too_small = matrix_compute_model(vec![(
        "matrix",
        matrix_execution_unit_config(Some((ACCUMULATOR_BYTES - 1) as u64), None),
    )]);
    assert_lowering_error(
        &too_small,
        &work,
        "capacity cannot hold logical output tile",
    );
}
