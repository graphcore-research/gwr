// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::program::syntax::ExprSpec;

#[test]
fn named_call_expression_round_trips_through_yaml() {
    let yaml = "hard_sigmoid: { args: [{ read: { tensor: A, indices: [i] } }], attrs: { alpha: 0.2, beta: 0.5 } }";
    let expr: ExprSpec = yaml_serde::from_str(yaml).unwrap();
    let emitted = yaml_serde::to_string(&expr).unwrap();
    assert!(emitted.contains("hard_sigmoid"));
    assert!(emitted.contains("alpha"));
    assert_eq!(expr, yaml_serde::from_str(&emitted).unwrap());
}

#[test]
fn operation_key_becomes_call_without_a_wrapper() {
    let yaml =
        "add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }]";
    let expr: ExprSpec = yaml_serde::from_str(yaml).unwrap();
    assert!(matches!(&expr, ExprSpec::Call { op, args, .. } if op == "add" && args.len() == 2));
    let emitted = yaml_serde::to_string(&expr).unwrap();
    assert!(emitted.starts_with("add:"), "{emitted}");
    assert_eq!(expr, yaml_serde::from_str(&emitted).unwrap());
}

#[test]
fn matrix_mac_has_a_distinct_expression_shape() {
    let yaml = "matrix_mac: { over: [k], args: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] }";
    let expr: ExprSpec = yaml_serde::from_str(yaml).unwrap();
    assert!(
        matches!(&expr, ExprSpec::MatrixMac { args, over, .. } if args.len() == 2 && over == &["k"])
    );
    let emitted = yaml_serde::to_string(&expr).unwrap();
    assert!(emitted.starts_with("matrix_mac:"), "{emitted}");
    assert_eq!(expr, yaml_serde::from_str(&emitted).unwrap());
    assert!(yaml_serde::from_str::<ExprSpec>("call: { op: sin, args: [] }").is_err());
}
