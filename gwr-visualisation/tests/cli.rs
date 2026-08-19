// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::process::Command;

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
fn data_script_preserves_a_proto_overlay_key_as_an_own_property() {
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
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
        .arg("--timetable")
        .arg("../gwr-timetable/examples/small.yaml")
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

    let data_script = temp.path().join("data.js");
    let output = Command::new("node")
        .arg("-e")
        .arg(
            r#"const fs = require("node:fs");
const vm = require("node:vm");
const context = { window: {} };
vm.runInNewContext(fs.readFileSync(process.argv[1], "utf8"), context);
if (!Object.hasOwn(context.window.GWR_VISUALISATION_DATA.overlay_metrics, "__proto__")) {
  process.exit(1);
}"#,
        )
        .arg(&data_script)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "data script did not preserve __proto__ as an own property: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_does_not_overwrite_an_input_used_as_an_output() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let timetable = output_dir.join("data.json");
    let contents = std::fs::read_to_string("../gwr-timetable/examples/small.yaml").unwrap();
    std::fs::write(&timetable, &contents).unwrap();

    assert_input_alias_is_rejected(&timetable, &output_dir, &timetable, &contents);
}

#[test]
fn cli_does_not_overwrite_a_hard_link_to_an_input() {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("timetable.yaml");
    let contents = std::fs::read_to_string("../gwr-timetable/examples/small.yaml").unwrap();
    std::fs::write(&timetable, &contents).unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let output_path = output_dir.join("data.json");
    std::fs::hard_link(&timetable, &output_path).unwrap();

    assert_input_alias_is_rejected(&timetable, &output_dir, &output_path, &contents);
}

#[cfg(unix)]
#[test]
fn cli_does_not_overwrite_an_input_through_an_output_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let timetable = temp.path().join("timetable.yaml");
    let contents = std::fs::read_to_string("../gwr-timetable/examples/small.yaml").unwrap();
    std::fs::write(&timetable, &contents).unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let output_path = output_dir.join("data.json");
    std::os::unix::fs::symlink(&timetable, &output_path).unwrap();

    assert_input_alias_is_rejected(&timetable, &output_dir, &output_path, &contents);
}

#[test]
fn cli_rejects_hard_links_between_output_files() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let first_output = output_dir.join("index.html");
    let second_output = output_dir.join("data.js");
    let contents = "existing report";
    std::fs::write(&first_output, contents).unwrap();
    std::fs::hard_link(&first_output, &second_output).unwrap();

    assert_output_alias_is_rejected(&output_dir, &first_output, &second_output, contents);
}

#[cfg(unix)]
#[test]
fn cli_rejects_symlinks_between_output_files() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let first_output = output_dir.join("index.html");
    let second_output = output_dir.join("data.js");
    let contents = "existing report";
    std::fs::write(&first_output, contents).unwrap();
    std::os::unix::fs::symlink(&first_output, &second_output).unwrap();

    assert_output_alias_is_rejected(&output_dir, &first_output, &second_output, contents);
}

#[cfg(unix)]
#[test]
fn cli_rejects_dangling_symlinks_between_output_files() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("report");
    std::fs::create_dir(&output_dir).unwrap();
    let first_output = output_dir.join("index.html");
    let second_output = output_dir.join("data.js");
    std::os::unix::fs::symlink("data.js", &first_output).unwrap();
    let existing_output = output_dir.join("style.css");
    let existing_contents = "existing report";
    std::fs::write(&existing_output, existing_contents).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
        .arg("--timetable")
        .arg("../gwr-timetable/examples/small.yaml")
        .arg("--out")
        .arg(&output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_link(&first_output).unwrap(),
        std::path::Path::new("data.js")
    );
    assert!(!second_output.exists());
    assert_eq!(
        std::fs::read_to_string(existing_output).unwrap(),
        existing_contents
    );
    assert!(!output_dir.join("data.json").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("InvalidInput"));
    assert!(stderr.contains(&format!(
        "Output file '{}' is a dangling symbolic link",
        first_output.display()
    )));
}

fn assert_input_alias_is_rejected(
    timetable: &std::path::Path,
    output_dir: &std::path::Path,
    output_path: &std::path::Path,
    contents: &str,
) {
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
        .arg("--timetable")
        .arg(timetable)
        .arg("--out")
        .arg(output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(timetable).unwrap(), contents);
    assert!(!output_dir.join("index.html").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("InvalidInput"));
    assert!(stderr.contains(&format!(
        "Output file '{}' aliases input file '{}'",
        output_path.display(),
        timetable.display()
    )));
}

fn assert_output_alias_is_rejected(
    output_dir: &std::path::Path,
    first_output: &std::path::Path,
    second_output: &std::path::Path,
    contents: &str,
) {
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
        .arg("--timetable")
        .arg("../gwr-timetable/examples/small.yaml")
        .arg("--out")
        .arg(output_dir)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(first_output).unwrap(), contents);
    assert_eq!(std::fs::read_to_string(second_output).unwrap(), contents);
    assert!(!output_dir.join("data.json").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("InvalidInput"));
    assert!(stderr.contains(&format!(
        "Output file '{}' aliases output file '{}'",
        second_output.display(),
        first_output.display()
    )));
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
  - id: tensor0
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [1] }
  - id: tensor1
    kind: tensor
    config: { addr: 4, dtype: fp32, shape: [1] }
edges:
  - { from: tensor0, to: tensor1, kind: data }
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
            .contains("Invalid edge from Tensor node 'tensor0' to Tensor node 'tensor1'")
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
        "benchmark-hooks.js",
    ];
    let mut previous_position = 0;
    for script in scripts {
        let contents = report.asset(script);
        assert!(!contents.contains("Tensor data"));
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
    let data_js = report.asset("data.js");
    assert_eq!(
        data_js.lines().count(),
        1,
        "data.js should use compact JSON"
    );
    assert!(data_js.len() < report.data_json.len());
    assert!(data_js.starts_with("window.GWR_VISUALISATION_DATA=JSON.parse("));

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
