// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;

use gwr_platform::catalog::{CatalogTarget, load_yaml_with_includes};
use gwr_platform::types::PlatformConfig;

#[test]
fn imported_profiles_can_be_selected_independently_by_pes() {
    let dir = directory();
    fs::write(dir.join("compute.yaml"), "operation_classes:\n  add: { operations: [add], arity: 2 }\ncompute_profiles:\n  slow: { engines: { alu: { capabilities: [{ class: add, ops_per_tick: 4 }] } } }\n  fast: { engines: { alu: { capabilities: [{ class: add, ops_per_tick: 16 }] } } }\n").unwrap();
    fs::write(dir.join("platform.yaml"), "includes: [compute.yaml]\nmemory_maps: []\nprocessing_elements:\n  - { name: pe0, memory_map: empty, config: { compute: slow } }\n  - { name: pe1, memory_map: empty, config: { compute: fast } }\n").unwrap();
    let merged =
        load_yaml_with_includes(&dir.join("platform.yaml"), CatalogTarget::Platform).unwrap();
    let platform: PlatformConfig = yaml_serde::from_str(&merged).unwrap();
    let pes = platform.processing_elements.unwrap();
    let first = pes[0]
        .effective_config(&platform.operation_classes, &platform.compute_profiles)
        .unwrap();
    let second = pes[1]
        .effective_config(&platform.operation_classes, &platform.compute_profiles)
        .unwrap();
    assert_eq!(
        first.compute.engines["alu"].capabilities[0].ops_per_tick,
        4.0
    );
    assert_eq!(
        second.compute.engines["alu"].capabilities[0].ops_per_tick,
        16.0
    );
}

#[test]
fn example_compute_profiles_have_independent_matrix_mac_engines() {
    let catalog =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/default_compute.yaml");
    let yaml = fs::read_to_string(catalog).unwrap();
    let catalog: yaml_serde::Value = yaml_serde::from_str(&yaml).unwrap();
    let classes = &catalog["operation_classes"];
    assert_eq!(classes["matrix"]["operations"][0], "matrix_mac");
    assert_eq!(classes["matrix"]["arity"], 2);
    assert_eq!(classes["matrix"]["flops_per_op"], 2);
    let profiles = &catalog["compute_profiles"];
    for (profile, rate) in [("basic_alu", 32), ("wide_alu", 256)] {
        let engines = &profiles[profile]["engines"];
        assert!(engines["alu"].is_mapping());
        assert_eq!(engines["matrix"]["capabilities"][0]["class"], "matrix");
        assert_eq!(engines["matrix"]["capabilities"][0]["ops_per_tick"], rate);
    }
}

#[test]
fn unknown_profile_is_rejected_and_inline_config_remains_valid() {
    let yaml = "operation_classes: { add: { operations: [add], arity: 2 } }\nmemory_maps: []\nprocessing_elements:\n  - { name: pe0, memory_map: empty, config: { compute: absent } }\n  - { name: pe1, memory_map: empty, config: { compute: { engines: { alu: { capabilities: [{ class: add, ops_per_tick: 8 }] } } } } }\n";
    let platform: PlatformConfig = yaml_serde::from_str(yaml).unwrap();
    let pes = platform.processing_elements.as_ref().unwrap();
    let error = pes[0]
        .effective_config(&platform.operation_classes, &BTreeMap::new())
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("Unknown compute profile 'absent'")
    );
    assert!(
        pes[1]
            .effective_config(&platform.operation_classes, &BTreeMap::new())
            .is_ok()
    );
}

#[test]
fn fabric_generator_uses_shared_default_catalog() {
    let dir = directory();
    let path = dir.join("platform.yaml");
    let output = Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
        .arg("--out")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated = fs::read_to_string(&path).unwrap();
    assert!(generated.contains("includes:"));
    assert!(generated.contains("  - \"default_compute.yaml\""));
    assert!(generated.contains("compute: \"basic_alu\""));
    assert!(!generated.contains("operation_classes:"));
    assert!(dir.join("default_compute.yaml").exists());
    let merged = load_yaml_with_includes(&path, CatalogTarget::Platform).unwrap();
    let parsed: PlatformConfig = yaml_serde::from_str(&merged).unwrap();
    assert_eq!(
        parsed.compute_profiles["basic_alu"].engines["alu"]
            .capabilities
            .len(),
        3
    );
    assert_eq!(
        parsed.compute_profiles["basic_alu"].engines["matrix"].capabilities[0].class,
        "matrix"
    );
}

#[test]
fn fabric_generator_rejects_output_overwriting_catalog() {
    for custom in [false, true] {
        let dir = directory();
        let catalog = dir.join(if custom {
            "compute.yaml"
        } else {
            "default_compute.yaml"
        });
        let contents = include_str!("../examples/default_compute.yaml");
        if custom {
            fs::write(&catalog, contents).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_gen-fabric"));
        command.arg("--out").arg(&catalog);
        if custom {
            command.arg("--compute-catalog").arg(&catalog);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "generator overwrote its catalog");
        assert!(String::from_utf8_lossy(&output.stderr).contains("catalog"));
        if custom {
            assert_eq!(fs::read_to_string(&catalog).unwrap(), contents);
        } else {
            assert!(!catalog.exists(), "rejected invocation created a catalog");
        }
    }
}

#[cfg(unix)]
#[test]
fn fabric_generator_rejects_output_symlink_to_catalog() {
    let dir = directory();
    let catalog = dir.join("compute.yaml");
    let path = dir.join("platform.yaml");
    let contents = include_str!("../examples/default_compute.yaml");
    fs::write(&catalog, contents).unwrap();
    std::os::unix::fs::symlink(&catalog, &path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
        .arg("--out")
        .arg(&path)
        .arg("--compute-catalog")
        .arg(&catalog)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "generator overwrote catalog through symlink"
    );
    assert_eq!(fs::read_to_string(&catalog).unwrap(), contents);
}

#[test]
fn fabric_generator_selects_custom_profile_and_rejects_unknown_name() {
    let dir = directory();
    let catalog = dir.join("compute.yaml");
    fs::write(&catalog, "operation_classes: { trig: { operations: [sin, cos], arity: 1 } }\ncompute_profiles: { transcendental: { engines: { special: { capabilities: [{ class: trig, ops_per_tick: 32 }] } } } }\n").unwrap();
    let path = dir.join("platform.yaml");
    let run = |profile: &str| {
        Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
            .args([
                "--out",
                path.to_str().unwrap(),
                "--compute-catalog",
                catalog.to_str().unwrap(),
                "--compute-profile",
                profile,
            ])
            .output()
            .unwrap()
    };
    let output = run("transcendental");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated = fs::read_to_string(&path).unwrap();
    assert!(generated.contains("compute: \"transcendental\""));
    assert!(generated.contains("  - \"compute.yaml\""));
    let merged = load_yaml_with_includes(&path, CatalogTarget::Platform).unwrap();
    let parsed: PlatformConfig = yaml_serde::from_str(&merged).unwrap();
    assert_eq!(
        parsed.compute_profiles["transcendental"].engines["special"].capabilities[0].ops_per_tick,
        32.0
    );
    let error = run("missing");
    assert!(!error.status.success());
    assert!(String::from_utf8_lossy(&error.stderr).contains("Unknown compute profile 'missing'"));
}

#[cfg(unix)]
#[test]
fn fabric_generator_rejects_dangling_output_symlink_before_creating_catalog() {
    let dir = directory();
    let catalog = dir.join("default_compute.yaml");
    let output_path = dir.join("platform.yaml");
    std::os::unix::fs::symlink(&catalog, &output_path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
        .arg("--out")
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "generator followed dangling output symlink"
    );
    assert!(!catalog.exists(), "rejected invocation created a catalog");
}

#[cfg(unix)]
#[test]
fn fabric_generator_resolves_includes_from_output_symlink_target_directory() {
    for custom in [false, true] {
        let dir = directory();
        let target_directory = dir.join("target");
        fs::create_dir(&target_directory).unwrap();
        let target = target_directory.join("platform.yaml");
        fs::write(&target, "").unwrap();
        let output_path = dir.join("platform.yaml");
        std::os::unix::fs::symlink(&target, &output_path).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_gen-fabric"));
        command.arg("--out").arg(&output_path);
        if custom {
            let catalog = dir.join("compute.yaml");
            fs::write(&catalog, include_str!("../examples/default_compute.yaml")).unwrap();
            command.arg("--compute-catalog").arg(catalog);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let merged = load_yaml_with_includes(&output_path, CatalogTarget::Platform).unwrap();
        let platform: PlatformConfig = yaml_serde::from_str(&merged).unwrap();
        assert!(platform.compute_profiles.contains_key("basic_alu"));
    }
}

#[test]
fn fabric_generator_quotes_profile_names_with_yaml_punctuation() {
    let dir = directory();
    let catalog = dir.join("compute.yaml");
    fs::write(&catalog, "operation_classes: { add: { operations: [add], arity: 2 } }\ncompute_profiles: { 'fast: special': { engines: { alu: { capabilities: [{ class: add, ops_per_tick: 32 }] } } } }\n").unwrap();
    let path = dir.join("platform.yaml");
    let output = Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
        .args([
            "--out",
            path.to_str().unwrap(),
            "--compute-catalog",
            catalog.to_str().unwrap(),
            "--compute-profile",
            "fast: special",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let merged = load_yaml_with_includes(&path, CatalogTarget::Platform).unwrap();
    let platform: PlatformConfig = yaml_serde::from_str(&merged).unwrap();
    assert_eq!(
        platform.processing_elements.as_ref().unwrap()[0]
            .config
            .compute
            .as_ref()
            .unwrap(),
        &gwr_platform::types::ComputeSelection::Named("fast: special".into())
    );
}

fn directory() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gwr-platform-catalog-{}-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}
