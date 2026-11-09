// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Unreferenced prospective loader controls; no compilation or execution yet.
//! Initial missing-function RED establishes API absence, not semantic
//! rejection.

use std::fs;
use std::path::PathBuf;

use gwr_platform::catalog::{
    CatalogTarget, load_yaml_with_includes, resolved_platform_json_from_yaml,
};
use serde_json::json;

#[test]
fn formatted_anchors_preserve_complete_independent_json() {
    let raw = "# caller formatting\nname: 'held platform'\nclock: 1000\nbase: &base {bytes_per_tick: 100.0, capacity: 4096}\nmemories: [{id: hbm, parameters: *base}]\nprocessing_elements: [{id: pe0, memory: hbm, enabled: true, optional: null}]\nplacement: {input: hbm, output: hbm}\nnotes: [caller, reference]\n";
    let expected = json!({
        "name": "held platform", "clock": 1000,
        "base": {"bytes_per_tick": 100.0, "capacity": 4096},
        "memories": [{"id": "hbm", "parameters": {"bytes_per_tick": 100.0, "capacity": 4096}}],
        "processing_elements": [{"id": "pe0", "memory": "hbm", "enabled": true, "optional": null}],
        "placement": {"input": "hbm", "output": "hbm"}, "notes": ["caller", "reference"]
    });
    let actual = resolved_platform_json_from_yaml(raw).unwrap();
    assert_eq!(actual, expected);
    assert!(actual["clock"].is_u64());
    assert!(actual["base"]["bytes_per_tick"].is_f64());
    assert!(actual["processing_elements"][0]["enabled"].is_boolean());
    assert!(actual["processing_elements"][0]["optional"].is_null());
    assert!(raw.starts_with("# caller formatting\n"));
}

#[test]
fn held_include_resolution_survives_original_file_mutation() {
    let fixture = Fixture::new();
    let root = fixture.0.join("root.yaml");
    let included = fixture.0.join("catalog.yaml");
    let root_bytes = "includes: [catalog.yaml]\nname: selected\nclock: 1000\nmemories: [{id: hbm, capacity: 4096}]\nprocessing_elements: [{id: pe0, profile: dense}]\nprograms: {discarded: {}}\n";
    let include_bytes = "operation_classes: {matrix: {arity: 2, operations: [matrix_mac]}}\ncompute_profiles: {dense: {capabilities: [{operation_class: matrix, input_types: [bf16, bf16], accumulator_type: fp32, rate: 100.0}]}}\n";
    fs::write(&root, root_bytes).unwrap();
    fs::write(&included, include_bytes).unwrap();
    let held = load_yaml_with_includes(&root, CatalogTarget::Platform).unwrap();
    let expected = json!({
        "name": "selected", "clock": 1000,
        "memories": [{"id": "hbm", "capacity": 4096}],
        "processing_elements": [{"id": "pe0", "profile": "dense"}],
        "operation_classes": {"matrix": {"arity": 2, "operations": ["matrix_mac"]}},
        "compute_profiles": {"dense": {"capabilities": [{"operation_class": "matrix", "input_types": ["bf16", "bf16"], "accumulator_type": "fp32", "rate": 100.0}]}}
    });
    assert_eq!(resolved_platform_json_from_yaml(&held).unwrap(), expected);
    assert_eq!(fs::read_to_string(&root).unwrap(), root_bytes);
    assert_eq!(fs::read_to_string(&included).unwrap(), include_bytes);
    fs::write(&included, include_bytes.replace("100.0", "200.0")).unwrap();
    fs::write(&root, root_bytes.replace("4096", "8192")).unwrap();
    assert_eq!(resolved_platform_json_from_yaml(&held).unwrap(), expected);
    let reloaded = load_yaml_with_includes(&root, CatalogTarget::Platform).unwrap();
    let mut changed = expected;
    changed["memories"][0]["capacity"] = json!(8192);
    changed["compute_profiles"]["dense"]["capabilities"][0]["rate"] = json!(200.0);
    assert_eq!(
        resolved_platform_json_from_yaml(&reloaded).unwrap(),
        changed
    );
    assert_ne!(held, reloaded);
}

#[test]
fn nonfinite_nested_values_never_become_json_null() {
    assert_eq!(
        resolved_platform_json_from_yaml("rate: 1.0\n").unwrap(),
        json!({"rate": 1.0})
    );
    for raw in [
        "rate: .nan\n",
        "rate: .inf\n",
        "rate: -.inf\n",
        "items: [{rate: .nan}]\n",
    ] {
        assert!(
            resolved_platform_json_from_yaml(raw).is_err(),
            "accepted {raw}"
        );
    }
}

#[test]
fn nonstring_keys_tags_and_malformed_yaml_are_rejected() {
    assert_eq!(
        resolved_platform_json_from_yaml("items: [{key: value}]\n").unwrap(),
        json!({"items": [{"key": "value"}]})
    );
    for raw in [
        "1: value\n",
        "items: [{false: value}]\n",
        "items: [{? [a, b]: value}]\n",
        "items: [!caller value]\n",
        "items: [unterminated\n",
    ] {
        assert!(
            resolved_platform_json_from_yaml(raw).is_err(),
            "accepted {raw}"
        );
    }
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "gwr-resolved-platform-json-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
