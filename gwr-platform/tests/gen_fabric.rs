// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs::read_to_string;
use std::path::Path;
use std::process::Command;

use gwr_models::processing_element::compute::CapabilityKind;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_platform::types::PlatformConfig;
use tempfile::tempdir;
use yaml_serde::from_str;

fn generate_platform(working_dir: &Path, output: &Path) {
    let result = Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
        .current_dir(working_dir)
        .arg("--out")
        .arg(output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "gen-fabric failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let yaml = read_to_string(working_dir.join(output)).unwrap();
    let platform: PlatformConfig = from_str(&yaml).unwrap();
    assert_eq!(platform.fabrics.as_ref().unwrap().len(), 1);
    assert_compute_rates(&platform, [16.0, 4.0, 16.0, 32.0]);
}

fn assert_compute_rates(platform: &PlatformConfig, rates: [f64; 4]) {
    for pe in platform.processing_elements.as_ref().unwrap() {
        let config = pe.effective_config(&platform.operation_classes).unwrap();
        for ((execution_unit, class), rate) in [
            ("alu", "add"),
            ("alu", "mul"),
            ("alu", "compare"),
            ("matrix", "matrix"),
        ]
        .into_iter()
        .zip(rates)
        {
            let capability = config.compute.execution_units[execution_unit]
                .capabilities
                .iter()
                .find(|capability| capability.class == class)
                .unwrap();
            assert_eq!(capability.ops_per_tick, rate, "{} {class}", pe.name);
            let expected_kind = if class == "matrix" {
                CapabilityKind::Matrix {
                    accumulator: DataType::Fp32,
                    matrix_service_tile: None,
                }
            } else {
                CapabilityKind::Scalar
            };
            assert_eq!(capability.kind, expected_kind);
        }
    }
}

#[test]
fn compute_rate_arguments_configure_every_generated_pe() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("platform.yaml");
    let result = Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
        .arg("--out")
        .arg(&output)
        .args([
            "--pe-adds-per-tick",
            "23.5",
            "--pe-muls-per-tick",
            "7.25",
            "--pe-compares-per-tick",
            "11.5",
            "--pe-matrix-macs-per-tick",
            "96.5",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let platform = from_str(&read_to_string(output).unwrap()).unwrap();
    assert_compute_rates(&platform, [23.5, 7.25, 11.5, 96.5]);
}

#[test]
fn creates_missing_output_directories() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("missing/parents/platform.yaml");
    generate_platform(dir.path(), &output);
}

#[test]
fn writes_output_in_existing_directory() {
    let dir = tempdir().unwrap();
    generate_platform(dir.path(), &dir.path().join("platform.yaml"));
}

#[test]
fn writes_output_with_only_a_filename() {
    let dir = tempdir().unwrap();
    generate_platform(dir.path(), Path::new("platform.yaml"));
}
