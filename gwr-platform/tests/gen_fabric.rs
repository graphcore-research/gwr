// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn creates_missing_output_directories() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("missing/parents/platform.yaml");
    generate_platform(dir.path(), &output);
}

#[test]
fn writes_output_in_existing_directory() {
    let dir = tempfile::tempdir().unwrap();
    generate_platform(dir.path(), &dir.path().join("platform.yaml"));
}

#[test]
fn writes_output_with_only_a_filename() {
    let dir = tempfile::tempdir().unwrap();
    generate_platform(dir.path(), Path::new("platform.yaml"));
}

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
    let yaml = fs::read_to_string(working_dir.join(output)).unwrap();
    let platform: gwr_platform::types::PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
    assert_eq!(platform.fabrics.unwrap().len(), 1);
}
