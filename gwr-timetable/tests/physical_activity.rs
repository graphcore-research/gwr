// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
#[test]
fn native_activity_exports_typed_completed_compute_without_traces() {
    let t = tempfile::tempdir().unwrap();
    let output = t.path().join("activity.json");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .args([
            "--platform",
            "examples/compute_region_pipeline_platform.yaml",
            "--timetable",
            "examples/compute_region_pipeline.yaml",
            "--activity-out",
        ])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert!(value["duration_s"].as_f64().unwrap() > 0.);
    let actions = value["pes"][0]["compute"].as_array().unwrap();
    assert!(
        actions.iter().any(|a| a["signature"]["op"] == "matrix_mac"
            && a["service_operations"].as_u64().unwrap() > 0)
    );
    assert_eq!(value["platform_digest"].as_str().unwrap().len(), 64);
}
