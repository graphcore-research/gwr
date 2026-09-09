// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::{SMALL_TIMETABLE, generator_command};

#[test]
fn rejects_structurally_invalid_timetable() {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("invalid.yaml");
    std::fs::write(
        &timetable,
        r"
nodes:
  - id: duplicate
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [1] }
  - id: duplicate
    kind: tensor
    config: { addr: 4, dtype: fp32, shape: [1] }
  - id: source_a
    kind: tensor
    config: { addr: 8, dtype: fp32, shape: [1] }
  - id: source_b
    kind: tensor
    config: { addr: 12, dtype: fp32, shape: [1] }
edges:
  - from: duplicate.invalid
    to: duplicate
    kind: data
  - from: source_a
    to: duplicate.0
    kind: data
  - from: source_b
    to: duplicate.0
    kind: data
",
    )
    .unwrap();
    let output_dir = temp.path().join("report");
    let output = generator_command()
        .arg("--timetable")
        .arg(&timetable)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&timetable.display().to_string()));
    assert!(stderr.contains("Duplicate Node ID 'duplicate'"));
    assert!(!output_dir.exists());
}

#[test]
fn identifies_invalid_optional_input_file() {
    let temp = tempfile::tempdir().unwrap();
    for (flag, filename, contents) in [
        ("--platform", "broken-platform.yaml", "fabrics: ["),
        ("--overlay", "broken-overlay.json", "{"),
    ] {
        let input = temp.path().join(filename);
        std::fs::write(&input, contents).unwrap();
        let output = generator_command()
            .arg("--timetable")
            .arg(SMALL_TIMETABLE)
            .arg(flag)
            .arg(&input)
            .arg("--out")
            .arg(temp.path().join(format!("report-{filename}")))
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(&input.display().to_string()),
            "stderr did not identify {filename}"
        );
    }
}

#[test]
fn rejects_overlapping_physical_memories() {
    let temp = tempfile::tempdir().unwrap();
    let platform = temp.path().join("overlapping-platform.yaml");
    std::fs::write(
        &platform,
        r"
memory_maps: []
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
      capacity_bytes: 1024
  - name: hbm1
    kind: hbm
    base_address: 512
    config:
      capacity_bytes: 1024
",
    )
    .unwrap();
    let output_dir = temp.path().join("report");
    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--platform")
        .arg(&platform)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&platform.display().to_string()));
    assert!(stderr.contains("Physical memory ranges overlap"));
    assert!(!output_dir.exists());
}

#[test]
fn rejects_invalid_platform_references() {
    let temp = tempfile::tempdir().unwrap();
    let platform = temp.path().join("invalid-references.yaml");
    std::fs::write(
        &platform,
        r"
memory_maps:
  - name: map0
    devices: [{ name: missing }]
",
    )
    .unwrap();
    let output_dir = temp.path().join("report");
    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--platform")
        .arg(&platform)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&platform.display().to_string()));
    assert!(stderr.contains("Unknown memory 'missing' in memory map 'map0'"));
    assert!(!output_dir.exists());
}

#[test]
fn rejects_invalid_data_edge() {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("invalid.yaml");
    std::fs::write(
        &timetable,
        r"
nodes:
  - id: tensor0
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [1] }
  - id: tensor1
    kind: tensor
    config: { addr: 4, dtype: fp32, shape: [1] }
edges:
  - { from: tensor0, to: tensor1, kind: data }
",
    )
    .unwrap();
    let output_dir = temp.path().join("report");
    let output = generator_command()
        .arg("--timetable")
        .arg(timetable)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Invalid edge from Tensor node 'tensor0' to Tensor node 'tensor1'")
    );
    assert!(!output_dir.exists());
}

#[test]
fn accepts_control_edges_without_tensor_indices() {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("control.yaml");
    std::fs::write(
        &timetable,
        r"
nodes:
  - id: first
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: []
  - id: second
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: []
edges:
  - from: first
    to: second
    kind: control
",
    )
    .unwrap();
    let output_dir = temp.path().join("report");
    let output = generator_command()
        .arg("--timetable")
        .arg(timetable)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "gwr-visualisation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_dir.join("data.json").exists());
}
