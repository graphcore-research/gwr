// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::{GeneratedReport, SMALL_TIMETABLE, generator_command};

#[test]
fn writes_static_bundle() {
    let report = GeneratedReport::generate();

    assert_script_bundle(&report);
    assert_report_controls(&report.index_html);
    assert_report_data(&report);
}

#[test]
fn data_script_preserves_a_proto_overlay_key() {
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

    let data_script = std::fs::read_to_string(temp.path().join("data.js")).unwrap();
    assert!(data_script.starts_with("window.GWR_VISUALISATION_DATA=JSON.parse("));
    assert!(data_script.contains(r#"\"__proto__\""#));
}

fn assert_script_bundle(report: &GeneratedReport) {
    let scripts = [
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
    ];
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
    assert!(report.asset("style.css").contains("color-scheme"));
    assert!(
        report
            .asset("data.js")
            .starts_with("window.GWR_VISUALISATION_DATA=JSON.parse(")
    );
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
