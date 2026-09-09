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
        std::fs::write(temp.path().join("d3.v7.min.js"), "stale asset").unwrap();
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
edges: []
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
  - { name: hbm0, kind: hbm, base_address: 0, config: { capacity_bytes: 1024 } }
  - { name: hbm1, kind: hbm, base_address: 512, config: { capacity_bytes: 1024 } }
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
  - id: invalid_add
    kind: compute
    op: add
    input_views: []
    output_views: []
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
            .contains("Compute node 'invalid_add': Add: 0 inputs found - expected 2")
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
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: []
  - id: second
    kind: compute
    op: { custom: { machine_ops: {} } }
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
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(output_dir.join("data.json")).unwrap())
            .unwrap();
    assert_eq!(data["graph"]["control_edges"][0]["from"], 0);
    assert_eq!(data["graph"]["control_edges"][0]["to"], 1);
}

fn assert_script_bundle(report: &GeneratedReport) {
    assert!(!report.asset("workspace.css").is_empty());
    assert!(report.index_html.contains("href=\"workspace.css\""));
    assert!(!report.index_html.contains("d3.v7.min.js"));
    assert!(!report.temp.path().join("d3.v7.min.js").exists());
    let scripts = [
        "data.js",
        "core.js",
        "palettes.js",
        "filters.js",
        "timetable-graph-layout.js",
        "timetable-graph-viewport.js",
        "timetable-graph.js",
        "pe-grid.js",
        "timetable.js",
        "tensors.js",
        "tensor-accesses.js",
        "memory.js",
        "relationships.js",
        "workspace-model.js",
        "workspace-settings.js",
        "workspace.js",
        "app.js",
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
    let viewport_js = report.asset("timetable-graph-viewport.js");
    assert!(viewport_js.contains("createCanvasViewport"));
    assert!(!viewport_js.contains("window.d3"));
    assert_renderer_assets(report);
}

fn assert_renderer_assets(report: &GeneratedReport) {
    let timetable_js = report.asset("timetable.js");
    assert!(timetable_js.contains("App.selectLayer(layer.name)"));
    assert!(timetable_js.contains("pe-overview-chart-header"));
    assert!(timetable_js.contains("data-pe-chart-sort"));
    assert!(timetable_js.contains("layerOverviewColumnConfiguration"));
    assert!(timetable_js.contains("data-layer-sort"));
    let core_js = report.asset("core.js");
    assert!(core_js.contains("\"timetable-graph-overview\""));
    assert!(core_js.contains("metric-strip-track"));
    assert!(core_js.contains("metric-traffic-row"));
    assert!(core_js.contains("data-pe-overview-measure"));
    assert!(core_js.contains("% of ${label}"));
    assert!(core_js.contains("overviewColumnControlsMarkup"));
    assert!(core_js.contains("bindOverviewColumnResizing"));
    assert!(core_js.contains("configuration.equalize()"));
    assert!(core_js.contains("data-column-resize"));
    assert!(core_js.contains("data-column-none"));
    let pe_grid_js = report.asset("pe-grid.js");
    assert!(pe_grid_js.contains("Timetable data"));
    assert!(pe_grid_js.contains("Metrics overlay"));
    assert!(report.asset("app.js").contains("setPeOverviewMeasure"));
    let tensors_js = report.asset("tensors.js");
    assert!(tensors_js.contains("renderTensorOverview"));
    assert!(tensors_js.contains("data-tensor-sort"));
    assert!(tensors_js.contains("reading-pes"));
    assert!(tensors_js.contains("tensor-detail-metrics"));
    assert!(tensors_js.contains("tensorOverviewColumnConfiguration"));
    assert!(tensors_js.contains("revealTensorOverviewSelection"));
    assert!(tensors_js.contains("scrollTensorOverviewSelectionIntoView"));
    let tensor_accesses_js = report.asset("tensor-accesses.js");
    assert!(tensor_accesses_js.contains("renderTensorAccesses"));
    assert!(tensor_accesses_js.contains("ACCESS_PAGE_ROW_LIMIT"));
    assert!(tensor_accesses_js.contains("tensor-view-dimension"));
    assert!(tensor_accesses_js.contains("restoreAccessScroll"));
    let graph_js = report.asset("timetable-graph.js");
    assert!(graph_js.contains("MAX_VISIBLE_NODES = 50_000"));
    assert!(!graph_js.contains("forceSimulation"));
    assert!(graph_js.contains("expandedLayers"));
    assert!(graph_js.contains("expandLayer"));
    assert!(graph_js.contains("expandVisible"));
    assert!(graph_js.contains("expandAll"));
    assert!(graph_js.contains("layoutTimetableGraph"));
    assert!(graph_js.contains("renderSelectedNode"));
    assert!(graph_js.contains("handleGraphKeydown"));
    assert!(graph_js.contains("drawGraphOverview"));
    assert!(graph_js.contains("centreGraphFromOverview"));
    assert!(graph_js.contains("fitGraphToOverviewRange"));
    assert!(graph_js.contains("navigation-highlighted"));
    assert!(graph_js.contains("setTimetableGraphEdgeDisplayMode"));
    assert!(graph_js.contains("dominant_machine_op"));
    let graph_layout_js = report.asset("timetable-graph-layout.js");
    assert!(graph_layout_js.contains("orderColumns"));
    assert!(graph_layout_js.contains("routeLinks"));
    assert!(graph_layout_js.contains("createSpatialIndex"));
    let memory_js = report.asset("memory.js");
    assert!(memory_js.contains("memoryOverviewColumnConfiguration"));
    assert!(memory_js.contains("data-memory-sort"));
    assert!(memory_js.contains("bindOverviewColumnResizing"));
    assert!(report.asset("index.html").contains("Layers overview"));
    assert!(
        report
            .asset("index.html")
            .contains("aria-keyshortcuts=\"ArrowUp ArrowDown ArrowLeft ArrowRight Enter + -\"")
    );
    assert!(
        report
            .asset("relationships.js")
            .contains("append(status, shell)")
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
        "data-view=\"tensor-overview\"",
        "id=\"tensor-overview\"",
        "data-view=\"tensor-accesses\"",
        "id=\"tensor-accesses\"",
        "id=\"memory-filter\"",
        "id=\"memory-filter-pattern\"",
        "id=\"workspace-shell\"",
        "id=\"workspace-navigation\"",
        "id=\"workspace-arrangement\"",
        "id=\"workspace-inspector-toggle\"",
        "id=\"workspace-reset\"",
        "data-view=\"timetable-graph\"",
        "id=\"timetable-graph\"",
        "id=\"timetable-graph-overview\"",
        "id=\"timetable-graph-expand-visible\"",
        "id=\"timetable-graph-expand-all\"",
        "id=\"timetable-graph-zoom-out\"",
        "id=\"timetable-graph-zoom-in\"",
        "id=\"timetable-graph-tensor-colour\"",
        "id=\"timetable-graph-group-size\"",
        "id=\"timetable-graph-group-size-legend\"",
        "id=\"timetable-graph-compute-lanes\"",
        "value=\"machine-op\"",
        "id=\"timetable-graph-edges-selection\"",
        "id=\"timetable-graph-edges-all\"",
        "id=\"timetable-graph-group-rows\"",
        "id=\"timetable-graph-tensor-legend\"",
        "<i class=\"layer\"></i>Layer",
        "<i class=\"group\"></i>Compute group",
        "data-view=\"selected-node\"",
        "id=\"selected-node\"",
        "id=\"pe-overview-measure\"",
        "id=\"pe-overview-chart\"",
        "id=\"pe-overview-grid\"",
        "id=\"pe-grid-encoding\"",
        "data-pe-grid-encoding=\"size\"",
        "id=\"pe-overview-legend\"",
        "id=\"pe-overview-scale\"",
        "<option value=\"global\" selected>Full timetable</option>",
        "id=\"pe-overview-fixed-maximum\"",
        "class=\"visual-state-legend\"",
        "id=\"pe-visual-state-legend\"",
        "data-filtered-state",
        "id=\"relationship-limit\"",
        "id=\"memory-visual-mode\"",
        "class=\"pe-overview-content\"",
        "id=\"memory-summary\"",
        "id=\"memories-overview\"",
    ] {
        assert!(index_html.contains(expected), "missing {expected}");
    }
    assert!(
        index_html.find("id=\"pe-overview-legend\"") < index_html.find("id=\"pe-overview-grid\""),
        "PE scale legend should precede the grid"
    );
}

fn assert_report_data(report: &GeneratedReport) {
    let data_js = report.asset("data.js");
    assert_eq!(
        data_js.lines().count(),
        1,
        "data.js should use compact JSON"
    );
    assert!(data_js.len() < report.data_json.len());

    assert_eq!(report.data["summary"]["compute_nodes"], 3);
    assert_eq!(report.data["summary"]["total_machine_ops"], "22579200");
    assert_eq!(report.data["summary"]["total_tensor_read_bytes"], "1204224");
    assert_eq!(report.data["summary"]["total_tensor_write_bytes"], "802816");
    assert_eq!(report.data["platform"]["processing_elements"], 15);
    assert!(report.data["layers"].is_array());
    assert_eq!(report.data["compute_nodes"].as_array().unwrap().len(), 3);
    assert_eq!(report.data["compute_nodes"][0]["id"], "pe_0_0_add");
    assert_eq!(report.data["compute_nodes"][0]["layer"], "layer 1");
    assert_eq!(report.data["compute_nodes"][0]["machine_ops"], "100352");
    assert_eq!(
        report.data["compute_nodes"][0]["dominant_machine_op"],
        "adds"
    );
    assert_eq!(
        report.data["graph"]["compute_groups"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(report.data["tensors"].is_array());
    assert!(report.data["pes"][0]["machine_ops_by_layer"].is_object());
    assert!(report.data["tensors"][0]["consumption_by_pe"][0]["by_layer"].is_object());

    let tensor = report.data["tensors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tensor| tensor["id"] == "tensor_0_0_to_0_1")
        .unwrap();
    assert_eq!(tensor["views"].as_array().unwrap().len(), 2);
    assert_eq!(tensor["element_bits"], 32);
    assert_eq!(tensor["views"][0]["byte_offset"], "0");
    assert_eq!(tensor["views"][0]["num_bytes"], "200704");
    assert_eq!(tensor["accesses"].as_array().unwrap().len(), 3);
    assert_eq!(tensor["accesses"][0]["direction"], "write");
    assert!(tensor["accesses"][0].get("view").is_none());
    assert_eq!(tensor["accesses"][1]["direction"], "read");
    assert_eq!(tensor["accesses"][1]["slot"], 0);
    assert_eq!(tensor["accesses"][1]["view"], 0);

    let machine_ops = report.data["machine_ops"].as_array().unwrap();
    assert_eq!(machine_ops.len(), 3);
    assert!(machine_ops.iter().any(|op| op["name"] == "adds"));
    assert!(machine_ops.iter().any(|op| op["label"] == "Multiplies"));
    assert!(machine_ops.iter().all(|op| {
        op["colour"]
            .as_str()
            .is_some_and(|colour| colour.starts_with('#'))
    }));
}
