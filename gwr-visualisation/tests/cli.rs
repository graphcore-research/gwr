// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::io::Read;
use std::process::Command;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::read::GzDecoder;

struct GeneratedReport {
    temp: tempfile::TempDir,
    index_html: String,
    data_json: String,
    data: serde_json::Value,
}

impl GeneratedReport {
    fn generate() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
            .arg("--timetable")
            .arg("../gwr-timetable/examples/small.yaml")
            .arg("--platform")
            .arg("../gwr-platform/examples/platform_4x4.yaml")
            .arg("--out")
            .arg(temp.path())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "gwr-visualisation failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let index_html = std::fs::read_to_string(temp.path().join("index.html")).unwrap();
        let data_json = std::fs::read_to_string(temp.path().join("data.json")).unwrap();
        let data = serde_json::from_str(&data_json).unwrap();
        Self {
            temp,
            index_html,
            data_json,
            data,
        }
    }

    fn asset(&self, name: &str) -> String {
        std::fs::read_to_string(self.temp.path().join(name))
            .unwrap_or_else(|error| panic!("unable to read generated {name}: {error}"))
    }
}

#[test]
fn cli_writes_static_bundle() {
    let report = GeneratedReport::generate();

    assert_script_bundle(&report);
    assert_report_controls(&report.index_html);
    assert_report_data(&report);
}

#[test]
fn cli_rejects_structurally_invalid_timetable() {
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
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
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
    assert!(stderr.contains("Unable to parse edge id 'duplicate.invalid'"));
    assert!(stderr.contains("input edge index 0 is connected more than once"));
    assert!(!output_dir.exists());
}

#[test]
fn cli_identifies_invalid_optional_input_file() {
    let temp = tempfile::tempdir().unwrap();
    for (flag, filename, contents) in [
        ("--platform", "broken-platform.yaml", "fabrics: ["),
        ("--overlay", "broken-overlay.json", "{"),
    ] {
        let input = temp.path().join(filename);
        std::fs::write(&input, contents).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
            .arg("--timetable")
            .arg("../gwr-timetable/examples/small.yaml")
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
fn cli_rejects_overlapping_physical_memories() {
    let temp = tempfile::tempdir().unwrap();
    let platform = temp.path().join("overlapping-platform.yaml");
    std::fs::write(
        &platform,
        r"
memory_maps: []
memories:
  - { name: hbm0, kind: hbm, base_address: 0, capacity_bytes: 1024 }
  - { name: hbm1, kind: hbm, base_address: 512, capacity_bytes: 1024 }
",
    )
    .unwrap();
    let output_dir = temp.path().join("report");
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
        .arg("--timetable")
        .arg("../gwr-timetable/examples/small.yaml")
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
fn cli_rejects_invalid_platform_references() {
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
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
        .arg("--timetable")
        .arg("../gwr-timetable/examples/small.yaml")
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
fn cli_rejects_semantically_invalid_timetable() {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("invalid.yaml");
    std::fs::write(
        &timetable,
        r"
nodes:
  - id: disconnected_load
    kind: memory
    op: load
    config: {}
edges: []
",
    )
    .unwrap();
    let output_dir = temp.path().join("report");
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
        .arg("--timetable")
        .arg(timetable)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("0 edges connect into Load node 'disconnected_load'")
    );
    assert!(!output_dir.exists());
}

#[test]
fn cli_accepts_control_edges_without_tensor_ports() {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("control.yaml");
    std::fs::write(
        &timetable,
        r"
nodes:
  - id: first
    kind: compute
    op: add
    input_views: []
    output_views: []
  - id: second
    kind: compute
    op: add
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
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
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

fn assert_script_bundle(report: &GeneratedReport) {
    let scripts = ["payload.js", "gwr_visualisation.js", "bootstrap.js"];
    let mut previous_position = 0;
    for script in scripts {
        let contents = report.asset(script);
        assert!(!contents.is_empty());
        let position = report
            .index_html
            .find(script)
            .unwrap_or_else(|| panic!("{script} missing from index.html"));
        assert!(
            position >= previous_position,
            "{script} is loaded out of dependency order"
        );
        previous_position = position;
    }
    assert!(
        report
            .asset("gwr_visualisation.js")
            .contains("wasm_bindgen")
    );
    let bootstrap = report.asset("bootstrap.js");
    assert!(bootstrap.contains("Unable to start visualisation"));
    assert!(bootstrap.contains("decodeBase64"));
    assert_compressed_payload(report);

    for legacy in [
        "data.js",
        "view-model.js",
        "core.js",
        "filters.js",
        "pe-grid.js",
        "timetable.js",
        "tensors.js",
        "memory.js",
        "relationships.js",
        "workspace.js",
        "app.js",
    ] {
        assert!(
            !report.temp.path().join(legacy).exists(),
            "legacy production script {legacy} was generated"
        );
        assert!(
            !report.index_html.contains(legacy),
            "legacy production script {legacy} was referenced"
        );
    }
}

fn assert_compressed_payload(report: &GeneratedReport) {
    let payload = report.asset("payload.js");
    let wasm = BASE64.decode(payload_value(&payload, "wasm")).unwrap();
    assert_eq!(&wasm[..4], b"\0asm");

    let compressed_data = BASE64.decode(payload_value(&payload, "data")).unwrap();
    let compressed_tensors = BASE64.decode(payload_value(&payload, "tensors")).unwrap();
    let mut browser_data = decompress_json(&compressed_data);
    browser_data["tensors"] = decompress_json(&compressed_tensors);
    assert_eq!(browser_data, report.data);
    assert!(compressed_data.len() + compressed_tensors.len() < report.data_json.len());
}

fn decompress_json(compressed: &[u8]) -> serde_json::Value {
    let mut decoder = GzDecoder::new(compressed);
    let mut json = String::new();
    decoder.read_to_string(&mut json).unwrap();
    serde_json::from_str(&json).unwrap()
}

fn payload_value<'a>(payload: &'a str, name: &str) -> &'a str {
    let prefix = format!("{name}:\"");
    let start = payload.find(&prefix).unwrap() + prefix.len();
    let end = payload[start..].find('"').unwrap() + start;
    &payload[start..end]
}

fn assert_report_controls(index_html: &str) {
    for expected in [
        "value=\"tensor-memory\"",
        "value=\"tensor-pe\"",
        "id=\"layer-filter\"",
        "id=\"layer-filter-pattern\"",
        "id=\"tensor-filter\"",
        "id=\"tensor-filter-pattern\"",
        "id=\"memory-filter\"",
        "id=\"memory-filter-pattern\"",
        "id=\"workspace-add-view\"",
        "id=\"workspace-reset\"",
        "data-preset=\"tensor\"",
        "id=\"pe-overview-measure\"",
        "id=\"pe-overview-chart\"",
        "id=\"pe-overview-grid\"",
        "id=\"pe-overview-legend\"",
        "class=\"pe-overview-content\"",
        "id=\"memory-summary\"",
        "id=\"memories-overview\"",
        "<option value=\"one\" selected>one column</option>",
        "class=\"views layout-one\"",
    ] {
        assert!(index_html.contains(expected), "missing {expected}");
    }
}

fn assert_report_data(report: &GeneratedReport) {
    assert_eq!(report.data["summary"]["compute_nodes"], 3);
    assert_eq!(report.data["summary"]["total_machine_ops"], "22579200");
    assert_eq!(report.data["summary"]["total_tensor_read_bytes"], "1204224");
    assert_eq!(report.data["summary"]["total_tensor_write_bytes"], "802816");
    assert_eq!(report.data["summary"]["data_edges"], 9);
    assert_eq!(report.data["platform"]["processing_elements"], 15);
    assert!(report.data["layers"].is_array());
    assert!(report.data["tensors"].is_array());
    assert!(report.data["pes"][0]["machine_ops_by_layer"].is_object());
    assert!(report.data["tensors"][0]["consumption_by_pe"][0]["by_layer"].is_object());
    assert!(report.data["tensors"][0]["consumption_by_pe"][0]["accesses"].is_array());

    let machine_ops = report.data["machine_ops"].as_array().unwrap();
    assert_eq!(machine_ops.len(), 3);
    assert!(machine_ops.iter().any(|op| op["name"] == "adds"));
    assert!(machine_ops.iter().any(|op| op["label"] == "Multiplies"));
}
