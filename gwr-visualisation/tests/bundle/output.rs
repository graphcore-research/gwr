// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use super::common::{
    GeneratedReport, SMALL_TIMETABLE, decompress_json, generator_command, payload_value,
};

#[test]
fn writes_static_bundle() {
    let report = GeneratedReport::generate();

    assert_script_bundle(&report);
    assert_report_controls(&report.index_html);
    assert_report_data(&report);
}

#[test]
fn compressed_payload_preserves_a_proto_overlay_key() {
    let temp = tempfile::tempdir().unwrap();
    let overlay = temp.path().join("overlay.json");
    std::fs::write(
        &overlay,
        r#"{
  "metrics": {
    "__proto__": { "label": "Prototype", "unit": null }
  }
}"#,
    )
    .unwrap();
    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--overlay")
        .arg(&overlay)
        .arg("--out")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gwr-visualisation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let payload = std::fs::read_to_string(temp.path().join("payload.js")).unwrap();
    let compressed_data = BASE64.decode(payload_value(&payload, "data")).unwrap();
    let data = decompress_json(&compressed_data);
    assert!(
        data["overlay_metrics"]
            .as_object()
            .unwrap()
            .contains_key("__proto__")
    );
}

#[test]
fn removes_data_file_from_older_report_bundles() {
    let temp = tempfile::tempdir().unwrap();
    let retired = ["data.js"];
    for name in retired {
        std::fs::write(temp.path().join(name), "old report data").unwrap();
    }

    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--out")
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "gwr-visualisation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in retired {
        assert!(!temp.path().join(name).exists(), "retained {name}");
    }
    assert!(temp.path().join("payload.js").is_file());
}

#[test]
fn leaves_the_bundle_unchanged_when_a_retired_path_is_a_directory() {
    let temp = tempfile::tempdir().unwrap();
    let index = temp.path().join("index.html");
    std::fs::write(&index, "existing report").unwrap();
    std::fs::create_dir(temp.path().join("data.js")).unwrap();

    let output = generator_command()
        .arg("--timetable")
        .arg(SMALL_TIMETABLE)
        .arg("--out")
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(index).unwrap(), "existing report");
    assert!(temp.path().join("data.js").is_dir());
    assert!(!temp.path().join("payload.js").exists());
}

fn assert_script_bundle(report: &GeneratedReport) {
    let scripts = ["payload.js", "bootstrap.js"];
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
    for script in [
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
        assert!(!report.asset(script).is_empty());
        assert!(!report.index_html.contains(script));
    }
    assert!(report.asset("style.css").contains("color-scheme"));
    assert!(report.asset("bootstrap.js").contains("decompressGzip"));
    assert_compressed_payload(report);
}

fn assert_compressed_payload(report: &GeneratedReport) {
    let payload = report.asset("payload.js");
    let compressed_data = BASE64.decode(payload_value(&payload, "data")).unwrap();
    let compressed_tensors = BASE64.decode(payload_value(&payload, "tensors")).unwrap();
    let mut browser_data = decompress_json(&compressed_data);
    browser_data["tensors"] = decompress_json(&compressed_tensors);
    assert_eq!(browser_data, report.data);
    assert!(compressed_data.len() + compressed_tensors.len() < report.data_json.len());
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
    assert!(report.data["tensors"][0]["reads_by_pe"][0]["by_layer"].is_object());
    assert!(report.data["tensors"][0]["reads_by_pe"][0]["transfers"].is_array());

    let transfer = &report.data["tensors"][0]["reads_by_pe"][0]["transfers"][0];
    assert!(transfer["access"]["strides"].is_array());
    assert!(transfer["access"]["num_access_bytes"].is_string());

    let machine_ops = report.data["machine_ops"].as_array().unwrap();
    assert_eq!(machine_ops.len(), 3);
    assert!(machine_ops.iter().any(|op| op["name"] == "adds"));
    assert!(machine_ops.iter().any(|op| op["label"] == "Multiplies"));
}
