// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn roofline_cli_writes_visualisation_metric_overlay() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_dir = tempfile::tempdir().unwrap();
    let overlay_path = output_dir.path().join("metrics.json");
    let output = Command::new(env!("CARGO_BIN_EXE_roofline-timetable"))
        .arg("--timetable")
        .arg(crate_dir.join("examples/small.yaml"))
        .arg("--platform")
        .arg(crate_dir.join("../gwr-platform/examples/platform_4x4.yaml"))
        .arg("--metric-overlay")
        .arg(&overlay_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "roofline-timetable failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Wrote metric overlay"));

    let overlay: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(overlay_path).unwrap()).unwrap();
    assert_eq!(overlay["metrics"].as_object().unwrap().len(), 5);
    assert_eq!(overlay["metrics"]["estimated_compute_ns"]["unit"], "ns");
    assert_eq!(overlay["metrics"]["estimated_memory_ns"]["unit"], "ns");
    assert_eq!(
        overlay["metrics"]["estimated_scheduled_finish_ns"]["unit"],
        "ns"
    );
    assert_eq!(
        overlay["metrics"]["estimated_compute_efficiency"]["unit"],
        "%"
    );
    assert_eq!(
        overlay["metrics"]["estimated_memory_efficiency"]["unit"],
        "%"
    );
    assert!(overlay["metrics_by_pe"]["pe_0_0"]["estimated_compute_ns"].is_number());
    assert!(overlay["metrics_by_pe"]["pe_0_0"]["estimated_compute_efficiency"].is_number());
    assert_eq!(
        overlay["metrics_by_pe"]["pe_0_2"]["estimated_compute_efficiency"],
        0.0
    );
    assert_eq!(
        overlay["metrics_by_pe"]["pe_0_2"]["estimated_memory_efficiency"],
        0.0
    );
}
