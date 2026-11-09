// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs::read_to_string;
use std::path::Path;
use std::process::Command;

use gwr_models::processing_element::compute::CapabilityKind;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_platform::types::PlatformConfig;
use tempfile::tempdir;
use yaml_serde::from_str;

fn generate_platform(working_dir: &Path, output: &Path, args: &[&str]) -> PlatformConfig {
    let result = Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
        .current_dir(working_dir)
        .arg("--out")
        .arg(output)
        .args(args)
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
    platform
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
    generate_platform(dir.path(), &output, &[]);
}

#[test]
fn writes_output_in_existing_directory() {
    let dir = tempdir().unwrap();
    generate_platform(dir.path(), &dir.path().join("platform.yaml"), &[]);
}

#[test]
fn writes_output_with_only_a_filename() {
    let dir = tempdir().unwrap();
    generate_platform(dir.path(), Path::new("platform.yaml"), &[]);
}

#[test]
fn cache_arguments_generate_the_requested_hardware_levels() {
    for (l2_kib, expected_caches) in [
        ("0", vec![("l1_0_0", 4096)]),
        ("16", vec![("l1_0_0", 4096), ("l2_0_0", 16384)]),
    ] {
        let dir = tempdir().unwrap();
        let platform = generate_platform(
            dir.path(),
            Path::new("platform.yaml"),
            &[
                "--num-columns",
                "2",
                "--num-rows",
                "1",
                "--num-hbms",
                "1",
                "--l1-kib",
                "4",
                "--l2-kib",
                l2_kib,
            ],
        );
        platform.validate().unwrap();
        let processing_elements = platform.processing_elements.as_ref().unwrap();
        assert_eq!(processing_elements.len(), 1);
        assert_eq!(processing_elements[0].name, "pe_0_0");
        let caches = platform.caches.as_ref().unwrap();
        assert_eq!(caches.len(), expected_caches.len());
        for (name, capacity_bytes) in expected_caches {
            let cache = caches.iter().find(|cache| cache.name == name).unwrap();
            let config = &cache.config;
            let actual_capacity = config.line_size_bytes.unwrap()
                * config.num_sets.unwrap()
                * config.num_ways.unwrap();
            assert_eq!(actual_capacity, capacity_bytes, "{name}");
        }
    }
}

#[test]
fn l2_cache_requires_an_l1_cache() {
    let dir = tempdir().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
        .current_dir(dir.path())
        .args([
            "--out",
            "platform.yaml",
            "--num-columns",
            "2",
            "--num-rows",
            "1",
            "--num-hbms",
            "1",
            "--l2-kib",
            "16",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("Cannot have an L2 cache without an L1 cache")
    );
    assert!(!dir.path().join("platform.yaml").exists());
}
