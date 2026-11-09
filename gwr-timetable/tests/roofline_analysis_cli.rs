// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::process::Command;

fn run_roofline(flags: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .args([
            "--roofline-analyze",
            "--platform",
            "examples/compute_region_platform.yaml",
            "--timetable",
            "examples/fused_region.yaml",
        ])
        .args(flags)
        .output()
        .unwrap()
}

#[test]
fn roofline_analysis_uses_info_logging_and_reports_totals_and_components() {
    let output = run_roofline(&["--stdout"]);

    assert!(
        output.status.success(),
        "gwr-timetable failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("INFO: Roofline analysis:"), "{stdout}");
    assert!(stdout.contains("Estimated simulated time: "), "{stdout}");
    assert!(
        stdout.contains("Processing elements: 1 | compute work:"),
        "{stdout}"
    );
    assert!(stdout.contains("Memory traffic: "), "{stdout}");
    assert!(stdout.contains("pe0: compute "), "{stdout}");
    assert!(stdout.contains("mem: "), "{stdout}");
    assert!(stdout.contains("resource-ticks"), "{stdout}");
    assert!(!stdout.contains("\"estimated_ticks\""), "{stdout}");
}

#[test]
fn roofline_analysis_reports_completion_without_an_info_tracker() {
    let output = run_roofline(&[]);

    assert!(
        output.status.success(),
        "gwr-timetable failed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("Ran simulation. Time now "), "{stdout}");
    assert!(!stdout.contains("INFO: Roofline analysis:"), "{stdout}");
}

#[test]
fn dump_stats_enables_roofline_info_output() {
    let output = run_roofline(&["--dump-stats"]);

    assert!(
        output.status.success(),
        "gwr-timetable failed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("INFO: Roofline analysis:"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn aligned_lsu_policy_is_explicit_and_reports_full_packet_payload() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("copy.yaml");
    std::fs::write(
        &path,
        r#"
nodes:
  - {id: A, kind: tensor, config: {addr: 4096, dtype: fp32, shape: [1]}}
  - id: copy
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      assignments:
        - {output: Y, axes: [i], expr: {read: {tensor: A, indices: [i]}}}
  - {id: Y, kind: tensor, config: {addr: 8192, dtype: fp32, shape: [1]}}
edges:
  - {from: A, to: copy, kind: data}
  - {from: copy, to: Y, kind: data}
"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .args([
            "--roofline-analyze",
            "--roofline-lsu-packets",
            "--stdout",
            "--platform",
            "examples/compute_region_platform.yaml",
            "--timetable",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Memory traffic: 64 bytes"));
}

#[test]
fn aligned_lsu_flag_requires_roofline_mode() {
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .arg("--roofline-lsu-packets")
        .output()
        .unwrap();
    assert!(!output.status.success());
}
