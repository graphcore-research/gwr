// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::program::syntax::{ExprSpec, ProgramSpec};
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
fn program_round_trip_preserves_local_outputs() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B, C]
outputs: [Y]
temporaries: { T: { dtype: fp32 } }
assignments:
  - output: T
    axes: [i]
    expr:
      add:
        - read: { tensor: A, indices: [i] }
        - read: { tensor: B, indices: [i] }
  - output: Y
    axes: [i]
    expr:
      mul:
        - read: { tensor: T, indices: [i] }
        - read: { tensor: C, indices: [i] }
",
    )
    .unwrap();
    assert_eq!(program.inputs, ["A", "B", "C"]);
    assert_eq!(program.outputs, ["Y"]);
    assert!(program.temporaries.contains_key("T"));
    assert_eq!(program.assignments.len(), 2);
    assert_eq!(program.assignments[0].output.as_deref(), Some("T"));
    assert!(matches!(&program.assignments[0].expr, ExprSpec::Call { op, .. } if op == "add"));
    assert!(matches!(&program.assignments[1].expr, ExprSpec::Call { op, .. } if op == "mul"));
    assert_eq!(
        from_str::<ProgramSpec>(&to_string(&program).unwrap()).unwrap(),
        program
    );
}

#[test]
fn program_round_trip_preserves_index_outputs() {
    let program: ProgramSpec = from_str(
        r"
inputs: [A]
outputs: [Y, I]
assignments:
  - output: Y
    indices_output: I
    axes: [o]
    expr:
      reduce:
        op: max
        over: [k]
        value:
          read: { tensor: A, indices: [o, k] }
",
    )
    .unwrap();
    assert_eq!(program.assignments[0].output.as_deref(), Some("Y"));
    assert_eq!(program.assignments[0].indices_output.as_deref(), Some("I"));
    assert_eq!(
        from_str::<ProgramSpec>(&to_string(&program).unwrap()).unwrap(),
        program
    );
}

#[test]
fn malformed_expressions_are_rejected() {
    for (name, expression, expected) in [
        ("call_args_are_mapping", "add: {wrong: []}", ""),
        (
            "two_expression_keys",
            "read: {tensor: A, indices: [i]}\nmul: [{read: {tensor: A, indices: [i]}}, {read: {tensor: A, indices: [i]}}]",
            "",
        ),
        ("reduce_missing_value", "reduce: {op: sum, over: [i]}", ""),
        (
            "matrix_missing_accumulator",
            "matrix_mac: {over: [k], args: [{read: {tensor: A, indices: [m,k]}}, {read: {tensor: B, indices: [k,n]}}]}",
            "missing field `accumulator`",
        ),
    ] {
        let error = from_str::<ExprSpec>(expression).expect_err(name);
        assert!(error.to_string().contains(expected), "{name}: {error}");
    }
}
