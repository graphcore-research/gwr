// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
// Pure engine accumulator admission; runner residency is a separate gate.
use std::collections::BTreeMap;

use gwr_models::processing_element::compute::{
    CapabilityConfig, ComputeConfig, ComputeModel, ComputeWork, EngineConfig, EngineId,
    ExecutionMode, MatrixServiceTile, MatrixWorkShape, OperationClass, Sparsity,
};
use gwr_models::processing_element::tensors::dtype::DataType;

fn mode() -> ExecutionMode {
    ExecutionMode {
        instruction: "tensor".into(),
        accumulator: DataType::Fp32,
        sparsity: Sparsity::Dense,
    }
}
fn cap(tile: Option<MatrixServiceTile>) -> CapabilityConfig {
    CapabilityConfig {
        class: "matrix".into(),
        input_dtypes: Some(vec![vec![DataType::Bf16; 2]]),
        ops_per_tick: 256.0,
        execution: Some(mode()),
        matrix_service_tile: tile,
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
fn engine(capacity: Option<u64>, tile: Option<MatrixServiceTile>) -> EngineConfig {
    EngineConfig {
        matrix_accumulator_capacity_bytes: capacity,
        capabilities: vec![cap(tile)],
    }
}
fn config(entries: Vec<(&str, EngineConfig)>) -> ComputeConfig {
    ComputeConfig {
        engines: entries
            .into_iter()
            .map(|(name, engine)| (name.into(), engine))
            .collect(),
    }
}
fn make_model(entries: Vec<(&str, EngineConfig)>) -> ComputeModel {
    ComputeModel::new(&config(entries), &classes()).unwrap()
}
fn work(m: usize, n: usize, k: usize) -> ComputeWork {
    ComputeWork::matrix_mac(vec![DataType::Bf16; 2], MatrixWorkShape { m, n, k })
        .unwrap()
        .with_execution(Some(mode()))
}
fn error(model: &ComputeModel, work: &ComputeWork, category: &str) {
    let message = model.lower(work).unwrap_err().to_string();
    assert!(message.contains(category), "{message}");
}

#[test]
fn strict_wire_and_none_omission_preserve_exact_declaration() {
    let none = engine(None, None);
    let yaml = yaml_serde::to_string(&none).unwrap();
    assert!(!yaml.contains("matrix_accumulator_capacity_bytes"));
    assert_eq!(yaml_serde::from_str::<EngineConfig>(&yaml).unwrap(), none);
    let explicit = engine(Some(256), None);
    let yaml = yaml_serde::to_string(&explicit).unwrap();
    assert_eq!(
        yaml_serde::from_str::<EngineConfig>(&yaml).unwrap(),
        explicit
    );
    for value in ["true", "1.5", "-1", "18446744073709551616"] {
        let invalid = format!(
            "matrix_accumulator_capacity_bytes: {value}\n{}",
            yaml_serde::to_string(&none).unwrap()
        );
        assert!(
            yaml_serde::from_str::<EngineConfig>(&invalid).is_err(),
            "{value}"
        );
    }
    assert!(yaml_serde::from_str::<EngineConfig>(&format!("{yaml}unknown: 1\n")).is_err());
}
#[test]
fn output_accumulator_bytes_ignore_k_and_service_padding() {
    let model = make_model(vec![("matrix", engine(Some(512), None))]);
    for (m, n, k, bytes) in [
        (8, 8, 17, 256),
        (8, 8, 4096, 256),
        (2, 32, 17, 256),
        (8, 16, 1, 512),
    ] {
        let plan = model.lower(&work(m, n, k)).unwrap();
        assert_eq!(plan.stages[0][0].matrix_accumulator_bytes, bytes);
        assert_eq!(
            plan.stages[0][0].machine_ops.by_operation["matrix_mac"],
            m * n * k
        );
    }
    error(
        &model,
        &work(usize::MAX / 8 + 1, 1, 1),
        "accumulator byte demand overflows",
    );
    assert_eq!(
        model.lower(&work(8, 8, 1)).unwrap().stages[0][0].matrix_accumulator_bytes,
        256
    );
}
#[test]
fn too_small_candidate_does_not_suppress_fit_or_unbounded_candidate() {
    let model = make_model(vec![
        ("a_small", engine(Some(128), None)),
        ("b_fit", engine(Some(256), None)),
        ("c_unbounded", engine(None, None)),
    ]);
    let plan = model.lower(&work(8, 8, 8)).unwrap();
    assert_eq!(
        plan.stages[0][0].alternatives,
        vec![(EngineId(1), 2), (EngineId(2), 2)]
    );
    assert_eq!(plan.stages[0][0].matrix_accumulator_bytes, 256);
    assert_eq!(
        plan.stages[0][0].machine_ops.by_operation["matrix_mac"],
        512
    );
    error(
        &make_model(vec![("only", engine(Some(128), None))]),
        &work(8, 8, 8),
        "capacity cannot hold logical output tile",
    );
    // Capacity filtering must not conceal inconsistent mathematical flop costs.
    let mut inconsistent = config(vec![
        ("a_small", engine(Some(128), None)),
        ("b_fit", engine(Some(256), None)),
    ]);
    inconsistent
        .engines
        .get_mut("a_small")
        .unwrap()
        .capabilities[0]
        .class = "other".into();
    let mut declarations = classes();
    let mut other = declarations["matrix"].clone();
    other.flops_per_op = 1;
    declarations.insert("other".into(), other);
    let inconsistent = ComputeModel::new(&inconsistent, &declarations).unwrap();
    error(&inconsistent, &work(8, 8, 8), "inconsistent flop costs");
}
#[test]
fn macro_alternatives_keep_logical_counts_and_residency_identical() {
    let padded = MatrixServiceTile {
        m: 16,
        n: 16,
        k: 16,
    };
    let model = make_model(vec![
        ("a_padded", engine(Some(256), Some(padded))),
        ("b_plain", engine(Some(256), None)),
    ]);
    let plan = model.lower(&work(2, 32, 16)).unwrap();
    let batch = &plan.stages[0][0];
    assert_eq!(
        batch.alternatives,
        vec![(EngineId(1), 4), (EngineId(0), 32)]
    );
    assert_eq!(batch.matrix_accumulator_bytes, 256);
    assert_eq!(batch.machine_ops.by_operation["matrix_mac"], 1024);
    assert_eq!(batch.machine_ops.flop_equivalents, 2048);
    let legacy = make_model(vec![("legacy", engine(None, None))])
        .lower(&work(2, 32, 16))
        .unwrap();
    assert_eq!(legacy.stages[0][0].matrix_accumulator_bytes, 0);
    assert_eq!(batch.machine_ops, legacy.stages[0][0].machine_ops);
}
#[test]
fn one_fixed_engine_uses_maximum_not_sum_of_sequential_demands() {
    let model = make_model(vec![("matrix", engine(Some(512), None))]);
    let plan = model
        .lower(&ComputeWork::parallel(vec![work(8, 8, 1), work(8, 16, 1)]))
        .unwrap();
    assert_eq!(plan.stages[0].len(), 1);
    let batch = &plan.stages[0][0];
    assert_eq!(batch.matrix_accumulator_bytes, 512);
    assert_eq!(batch.machine_ops.by_operation["matrix_mac"], 192);
    assert_eq!(batch.ticks, 1);
}
#[test]
fn missing_shape_wrong_mode_and_count_mismatch_have_intended_errors() {
    let model = make_model(vec![("matrix", engine(Some(256), None))]);
    model.lower(&work(8, 8, 1)).unwrap();
    error(
        &model,
        &ComputeWork::operation("matrix_mac", vec![DataType::Bf16; 2], 64)
            .with_execution(Some(mode())),
        "capacity requires logical matrix shape",
    );
    error(
        &model,
        &work(8, 8, 1).with_execution(None),
        "unsupported capability",
    );
    let mut mismatch = work(8, 8, 1);
    mismatch.stages[0].operations[0].count += 1;
    error(&model, &mismatch, "shape does not match logical work");
}
#[test]
fn capacity_rejects_zero_unqualified_sparse_or_nonmatrix_declarations() {
    let valid = config(vec![("matrix", engine(Some(256), None))]);
    ComputeModel::new(&valid, &classes()).unwrap();
    let before = valid.clone();
    for kind in 0..5 {
        let mut bad = valid.clone();
        let engine = bad.engines.get_mut("matrix").unwrap();
        let category = if kind == 0 {
            engine.matrix_accumulator_capacity_bytes = Some(0);
            "capacity must be positive"
        } else {
            match kind {
                1 => engine.capabilities[0].input_dtypes = None,
                2 => engine.capabilities[0].execution = None,
                3 => {
                    engine.capabilities[0].execution.as_mut().unwrap().sparsity =
                        Sparsity::Structured
                }
                _ => engine.capabilities.clear(),
            };
            if kind == 4 {
                "name and capabilities"
            } else {
                "requires exact dense matrix"
            }
        };
        let message = ComputeModel::new(&bad, &classes()).unwrap_err().to_string();
        assert!(message.contains(category), "{message}");
    }
    let scalar_classes = BTreeMap::from([(
        "matrix".into(),
        OperationClass {
            operations: vec!["add".into()],
            arity: 2,
            flops_per_op: 1,
        },
    )]);
    let message = ComputeModel::new(&valid, &scalar_classes)
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("requires a matrix declaration"),
        "{message}"
    );
    assert_eq!(valid, before);
    ComputeModel::new(&valid, &classes()).unwrap();
}
