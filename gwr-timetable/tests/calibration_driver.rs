// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::process::Command;

#[test]
#[ignore = "requires cargo build --release -p gwr-timetable --example calibrate"]
fn calibration_driver_reports_both_modes_and_nonunit_clock() {
    let driver = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/release/examples/calibrate");
    assert!(
        driver.exists(),
        "build calibration example before running this opt-in test"
    );
    for mode in ["simulated", "aggregate-roofline"] {
        let execute = |period: &str| {
            let output = Command::new(&driver)
                .args([
                    mode,
                    "examples/compute_region_platform.yaml",
                    "examples/fused_region.yaml",
                    period,
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
        };
        let normal = execute("1000");
        let slow = execute("2000");
        if mode == "simulated" {
            assert_eq!(normal["compute"][0]["lsu"]["issued_read_packets"], 6);
            assert_eq!(normal["compute"][0]["lsu"]["outstanding_packets"], 0);
        }
        assert_eq!(
            normal["memory_read"],
            if mode == "simulated" { 192 } else { 96 }
        );
        assert_eq!(
            normal["memory_written"],
            if mode == "simulated" { 64 } else { 32 }
        );
        assert_eq!(
            slow["simulated_time_ns"].as_f64().unwrap(),
            2.0 * normal["simulated_time_ns"].as_f64().unwrap()
        );
    }
}
