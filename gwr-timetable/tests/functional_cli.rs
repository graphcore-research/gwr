// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![cfg(feature = "functional")]

use std::process::{Command, Output};

fn run_fixture(expected: &str, flags: &[&str]) -> Output {
    run_data_fixture("[{address: 4096, values: [3, -2]}]", expected, flags, &[])
}

fn run_data_fixture(
    inputs: &str,
    expected: &str,
    flags: &[&str],
    files: &[(&str, &[u8])],
) -> Output {
    run_typed_fixture("fp32", 2, inputs, expected, flags, files)
}

fn run_typed_fixture(
    dtype: &str,
    count: usize,
    inputs: &str,
    expected: &str,
    flags: &[&str],
    files: &[(&str, &[u8])],
) -> Output {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("copy.yaml");
    let fixture = temp.path().join("data.yaml");
    for (name, bytes) in files {
        std::fs::write(temp.path().join(name), bytes).unwrap();
    }
    std::fs::write(
        &timetable,
        r"
nodes:
  - {id: A, kind: tensor, config: {addr: 4096, dtype: fp32, shape: [2]}}
  - id: copy
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      assignments:
        - {output: Y, axes: [i], expr: {read: {tensor: A, indices: [i]}}}
  - {id: Y, kind: tensor, config: {addr: 8192, dtype: fp32, shape: [2]}}
edges:
  - {from: A, to: copy.0, kind: data}
  - {from: copy.0, to: Y, kind: data}
"
        .replace("dtype: fp32", &format!("dtype: {dtype}"))
        .replace("shape: [2]", &format!("shape: [{count}]")),
    )
    .unwrap();
    std::fs::write(
        &fixture,
        format!("inputs: {inputs}\nchecks: {expected}\natol: 0\nrtol: 0\n"),
    )
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .args([
            "--platform",
            "examples/compute_region_platform.yaml",
            "--timetable",
        ])
        .arg(timetable)
        .arg("--functional-data")
        .arg(fixture)
        .arg("--error-mermaid")
        .arg(temp.path().join("error.mmd"))
        .args(flags)
        .output()
        .unwrap()
}

#[test]
fn checks_loaded_data_after_simulated_copy() {
    let output = run_fixture("[{address: 8192, values: [3, -2]}]", &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Functional checks passed: 2 elements")
    );
}

#[test]
fn incorrect_reference_fails_the_command() {
    let output = run_fixture("[{address: 8192, values: [3, 2]}]", &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mismatch"));
}

#[test]
fn empty_checks_are_rejected() {
    let output = run_fixture("[]", &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("at least one"));
}

#[test]
fn roofline_analysis_cannot_skip_functional_checks() {
    let output = run_fixture(
        "[{address: 8192, values: [3, -2]}]",
        &["--roofline-analyze"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn binary_inputs_and_checks_resolve_relative_to_manifest() {
    let bytes = [3.0_f32, -2.0]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect::<Vec<_>>();
    let output = run_data_fixture(
        "[{address: 4096, file: input.f32}]",
        "[{address: 8192, file: expected.f32}]",
        &[],
        &[("input.f32", &bytes), ("expected.f32", &bytes)],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Functional checks passed: 2 elements")
    );
}
#[test]
fn malformed_binary_length_and_ambiguous_sources_fail() {
    let output = run_data_fixture(
        "[{address: 4096, file: input.f32}]",
        "[{address: 8192, values: [3, -2]}]",
        &[],
        &[("input.f32", &[0, 1, 2])],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("multiple of four"));
    let output = run_data_fixture(
        "[{address: 4096, values: [3, -2], file: input.f32}]",
        "[{address: 8192, values: [3, -2]}]",
        &[],
        &[],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exactly one"));
}

#[test]
fn missing_binary_file_fails_before_execution() {
    let output = run_data_fixture(
        "[{address: 4096, file: absent.f32}]",
        "[{address: 8192, values: [3, -2]}]",
        &[],
        &[],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("absent.f32"));
}

#[test]
fn typed_inline_values_preserve_wide_integers_and_booleans() {
    for (dtype, values) in [
        ("uint64", "[9007199254740993, 18446744073709551615]"),
        ("int64", "[-9223372036854775808, 9007199254740993]"),
        ("bool", "[true, false]"),
        ("fp16", "[1.5, -2.25]"),
        ("bf16", "[1.5, -2.25]"),
        ("fp8", "[1.5, -2.25]"),
        ("fp4", "[1.5, -2]"),
    ] {
        let input = format!("[{{address: 4096, dtype: {dtype}, values: {values}}}]");
        let expected = format!("[{{address: 8192, dtype: {dtype}, values: {values}}}]");
        let output = run_typed_fixture(dtype, 2, &input, &expected, &[], &[]);
        assert!(
            output.status.success(),
            "{dtype}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn raw_bits_and_packed_files_keep_the_exact_encoding() {
    for (dtype, count, bits, bytes) in [
        ("fp8", 2, "[0x38, 0xb8]", vec![0x38, 0xb8]),
        ("fp16", 2, "[0x3c00, 0xbc00]", vec![0, 0x3c, 0, 0xbc]),
        ("int4", 3, "[0xf, 0x1, 0x8]", vec![0x1f, 0x08]),
        ("bool", 3, "[1, 0, 1]", vec![5]),
    ] {
        let expected = format!("[{{address: 8192, dtype: {dtype}, bits: {bits}}}]");
        for input in [
            format!("[{{address: 4096, dtype: {dtype}, bits: {bits}}}]"),
            format!("[{{address: 4096, dtype: {dtype}, file: input.bin, count: {count}}}]"),
        ] {
            let output = run_typed_fixture(
                dtype,
                count,
                &input,
                &expected,
                &[],
                &[("input.bin", &bytes)],
            );
            assert!(
                output.status.success(),
                "{dtype}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn exact_integer_checks_ignore_floating_tolerance() {
    let output = run_typed_fixture(
        "uint64",
        2,
        "[{address: 4096, dtype: uint64, values: [9007199254740993, 18446744073709551615]}]",
        "[{address: 8192, dtype: uint64, values: [9007199254740992, 18446744073709551615]}]",
        &[],
        &[],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mismatch"));
}

#[test]
fn invalid_typed_sources_fail_before_simulation() {
    for (input, error) in [
        ("{dtype: int4, values: [8, 0]}", "outside"),
        ("{dtype: fp4, bits: [16, 0]}", "storage width"),
        (
            "{dtype: bool, values: [true, false], bits: [1, 0]}",
            "exactly one",
        ),
        ("{dtype: bool, file: input.bin}", "require count"),
        ("{dtype: bool, file: input.bin, count: 9}", "length"),
        (
            "{dtype: bool, values: [true, false], count: 2}",
            "only valid with file",
        ),
    ] {
        let input = input.replacen('{', "{address: 4096, ", 1);
        let output = run_typed_fixture(
            "bool",
            2,
            &format!("[{input}]"),
            "[{address: 8192, dtype: bool, values: [true, false]}]",
            &[],
            &[("input.bin", &[1])],
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(error),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn missing_meaningful_packed_bits_fail_instead_of_becoming_zero() {
    let output = run_typed_fixture(
        "bool",
        2,
        "[{address: 4096, dtype: bool, bits: [1]}]",
        "[{address: 8192, dtype: bool, bits: [1, 0]}]",
        &[],
        &[],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("uninitialized"));
}
