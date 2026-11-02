// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::program::syntax::ExprSpec;
use yaml_serde::{from_str, to_string};

#[test]
fn operation_key_becomes_call_without_a_wrapper() {
    let yaml = r"
add:
  - read: { tensor: A, indices: [i] }
  - read: { tensor: B, indices: [i] }
";
    let expr: ExprSpec = from_str(yaml).unwrap();
    assert!(matches!(&expr, ExprSpec::Call { op, args, .. } if op == "add" && args.len() == 2));
    let emitted = to_string(&expr).unwrap();
    assert!(emitted.starts_with("add:"), "{emitted}");
    assert_eq!(expr, from_str(&emitted).unwrap());
}

#[test]
fn matrix_mac_parses_as_a_dedicated_expression() {
    let yaml = r"
matrix_mac:
  accumulator: fp32
  over: [k]
  args:
    - read: { tensor: A, indices: [m, k] }
    - read: { tensor: B, indices: [k, n] }
";
    let expr: ExprSpec = from_str(yaml).unwrap();
    assert!(
        matches!(&expr, ExprSpec::MatrixMac { args, over, .. } if args.len() == 2 && over == &["k"])
    );
    let emitted = to_string(&expr).unwrap();
    assert!(emitted.starts_with("matrix_mac:"), "{emitted}");
    assert_eq!(expr, from_str(&emitted).unwrap());
}

#[test]
fn matrix_mac_requires_an_accumulator() {
    let yaml = r"
matrix_mac:
  over: [k]
  args:
    - read: { tensor: A, indices: [m, k] }
    - read: { tensor: B, indices: [k, n] }
";
    assert!(
        from_str::<ExprSpec>(yaml)
            .unwrap_err()
            .to_string()
            .contains("missing field `accumulator`")
    );
}
