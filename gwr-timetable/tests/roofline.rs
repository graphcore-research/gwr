// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::analysis::cache::CacheModel;
use gwr_timetable::analysis::memory::{BandwidthGraph, WidestPathCache, resource_bytes_per_cycle};
use gwr_timetable::analysis::overlay::MetricOverlay;
use gwr_timetable::analysis::pe::{
    aggregate_pe_rooflines, compute_node_rooflines, critical_path_analysis, schedule_pe_activities,
};
use gwr_timetable::analysis::roofline::{RooflineAnalysisOptions, RooflineAnalyzer};
use gwr_timetable::timetable_file::TimetableFile;
use gwr_timetable::{Timetable, format_machine_ops};

const PLATFORM_YAML: &str = "
memory_maps:
  - name: default
    devices:
      - name: hbm0

processing_elements:
  - name: pe0
    memory_map: default
    config:
      lsu_access_bytes: 10
      overhead_size_bytes: 0
      adds_per_tick: 1.0
      muls_per_tick: 1.0
      compares_per_tick: 1.0
  - name: pe1
    memory_map: default
    config:
      lsu_access_bytes: 10
      overhead_size_bytes: 0
      adds_per_tick: 1.0
      muls_per_tick: 1.0
      compares_per_tick: 1.0

fabrics:
  - name: fabric0
    kind: functional
    columns: 3
    rows: 1
    port_bits_per_tick: 80

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1GiB
    bw_bytes_per_cycle: 10

connections:
  - connect:
      - pe.pe0
      - fabric.fabric0@(0,0)
  - connect:
      - pe.pe1
      - fabric.fabric0@(1,0)
  - connect:
      - mem.hbm0
      - fabric.fabric0@(2,0)
";

const CHAIN_TIMETABLE_YAML: &str = "
nodes:
  - id: in0
    kind: tensor
    config:
      addr: 0x1000
      dtype: fp8
      shape: [10]
  - id: in1
    kind: tensor
    config:
      addr: 0x2000
      dtype: fp8
      shape: [10]
  - id: add0
    kind: compute
    op: add
    pe: pe0
    input_views:
      -
      -
    output_views:
      -
  - id: mid
    kind: tensor
    config:
      addr: 0x3000
      dtype: fp8
      shape: [10]
  - id: in2
    kind: tensor
    config:
      addr: 0x4000
      dtype: fp8
      shape: [10]
  - id: add1
    kind: compute
    op: add
    pe: pe1
    input_views:
      -
      -
    output_views:
      -
  - id: out
    kind: tensor
    config:
      addr: 0x5000
      dtype: fp8
      shape: [10]

edges:
  - from: in0
    to: add0
    kind: data
  - from: in1
    to: add0
    kind: data
  - from: add0
    to: mid
    kind: data
  - from: mid
    to: add1
    kind: data
  - from: in2
    to: add1
    kind: data
  - from: add1
    to: out
    kind: data
";

const PARALLEL_TIMETABLE_YAML: &str = "
nodes:
  - id: a_in0
    kind: tensor
    config:
      addr: 0x1000
      dtype: fp8
      shape: [10]
  - id: a_in1
    kind: tensor
    config:
      addr: 0x2000
      dtype: fp8
      shape: [10]
  - id: add0
    kind: compute
    op: add
    pe: pe0
    input_views:
      -
      -
    output_views:
      -
  - id: a_out
    kind: tensor
    config:
      addr: 0x3000
      dtype: fp8
      shape: [10]
  - id: b_in0
    kind: tensor
    config:
      addr: 0x4000
      dtype: fp8
      shape: [10]
  - id: b_in1
    kind: tensor
    config:
      addr: 0x5000
      dtype: fp8
      shape: [10]
  - id: add1
    kind: compute
    op: add
    pe: pe1
    input_views:
      -
      -
    output_views:
      -
  - id: b_out
    kind: tensor
    config:
      addr: 0x6000
      dtype: fp8
      shape: [10]

edges:
  - from: a_in0
    to: add0
    kind: data
  - from: a_in1
    to: add0
    kind: data
  - from: add0
    to: a_out
    kind: data
  - from: b_in0
    to: add1
    kind: data
  - from: b_in1
    to: add1
    kind: data
  - from: add1
    to: b_out
    kind: data
";

const SHARED_CACHE_GEMM_PLATFORM_YAML: &str = "
memory_maps:
  - name: default
    devices:
      - name: hbm0

processing_elements:
  - name: pe0
    memory_map: default
    config:
      lsu_access_bytes: 4
      overhead_size_bytes: 0
      adds_per_tick: 1.0
      muls_per_tick: 1.0
  - name: pe1
    memory_map: default
    config:
      lsu_access_bytes: 4
      overhead_size_bytes: 0
      adds_per_tick: 1.0
      muls_per_tick: 1.0

caches:
  - name: shared
    config:
      bw_bytes_per_cycle: 16
      line_size_bytes: 4
      num_sets: 16
      num_ways: 4

fabrics:
  - name: fabric0
    kind: functional
    columns: 3
    rows: 1
    port_bits_per_tick: 128

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1GiB
    bw_bytes_per_cycle: 4

connections:
  - connect:
      - pe.pe0
      - fabric.fabric0@(0,0)
  - connect:
      - pe.pe1
      - fabric.fabric0@(1,0)
  - connect:
      - fabric.fabric0@(2,0)
      - cache.shared.dev
  - connect:
      - cache.shared.mem
      - mem.hbm0
";

const SHARED_CACHE_GEMM_TIMETABLE_YAML: &str = "
nodes:
  - id: a0
    kind: tensor
    config:
      addr: 0x1000
      dtype: fp8
      shape: [2, 2]
  - id: shared_b
    kind: tensor
    config:
      addr: 0x2000
      dtype: fp8
      shape: [2, 2]
  - id: gemm0
    kind: compute
    op: gemm
    pe: pe0
    input_views:
      -
      -
    output_views:
      -
  - id: out0
    kind: tensor
    config:
      addr: 0x3000
      dtype: fp8
      shape: [2, 2]
  - id: a1
    kind: tensor
    config:
      addr: 0x4000
      dtype: fp8
      shape: [2, 2]
  - id: gemm1
    kind: compute
    op: gemm
    pe: pe1
    input_views:
      -
      -
    output_views:
      -
  - id: out1
    kind: tensor
    config:
      addr: 0x5000
      dtype: fp8
      shape: [2, 2]

edges:
  - from: a0
    to: gemm0
    kind: data
  - from: shared_b
    to: gemm0
    kind: data
  - from: gemm0
    to: out0
    kind: data
  - from: a1
    to: gemm1
    kind: data
  - from: shared_b
    to: gemm1
    kind: data
  - from: gemm1
    to: out1
    kind: data
";

const SPLIT_MEMORY_PLATFORM_YAML: &str = "
memory_maps:
  - name: default
    devices:
      - name: hbm0
      - name: hbm1

processing_elements:
  - name: pe0
    memory_map: default
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 16B
  - name: hbm1
    kind: hbm
    base_address: 16
    capacity_bytes: 16B
";

const INACCESSIBLE_VIEW_PLATFORM_YAML: &str = "
memory_maps:
  - name: default
    devices:
      - name: hbm0

processing_elements:
  - name: pe0
    memory_map: default
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 16B
  - name: hbm1
    kind: hbm
    base_address: 16
    capacity_bytes: 16B
";

const SPANNING_TIMETABLE_YAML: &str = "
nodes:
  - id: in0
    kind: tensor
    config:
      addr: 8
      dtype: fp8
      shape: [16]
  - id: in1
    kind: tensor
    config:
      addr: 0
      dtype: fp8
      shape: [16]
  - id: add0
    kind: compute
    op: add
    pe: pe0
    input_views:
      -
      -
    output_views:
      -
  - id: out
    kind: tensor
    config:
      addr: 0
      dtype: fp8
      shape: [16]

edges:
  - from: in0
    to: add0
    kind: data
  - from: in1
    to: add0
    kind: data
  - from: add0
    to: out
    kind: data
";

const INACCESSIBLE_VIEW_TIMETABLE_YAML: &str = "
nodes:
  - id: in0
    kind: tensor
    config:
      addr: 0
      dtype: fp8
      shape: [32]
  - id: in1
    kind: tensor
    config:
      addr: 0
      dtype: fp8
      shape: [16]
  - id: add0
    kind: compute
    op: add
    pe: pe0
    input_views:
      - offsets: [16]
        shape: [16]
      -
    output_views:
      -
  - id: out
    kind: tensor
    config:
      addr: 0
      dtype: fp8
      shape: [16]

edges:
  - from: in0
    to: add0
    kind: data
  - from: in1
    to: add0
    kind: data
  - from: add0
    to: out
    kind: data
";

fn build_platform_and_graph_from(
    yaml: &str,
) -> (
    Rc<Platform>,
    PlatformConfig,
    BandwidthGraph,
    WidestPathCache,
) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, yaml).unwrap());
    let cfg = serde_yaml::from_str::<PlatformConfig>(yaml).unwrap();
    let bytes_per_cycle = resource_bytes_per_cycle(&platform).unwrap();
    let graph = BandwidthGraph::build(&cfg, &bytes_per_cycle).unwrap();
    (platform, cfg, graph, WidestPathCache::default())
}

fn build_platform_and_graph() -> (
    Rc<Platform>,
    PlatformConfig,
    BandwidthGraph,
    WidestPathCache,
) {
    build_platform_and_graph_from(PLATFORM_YAML)
}

fn build_timetable(platform: &Rc<Platform>, yaml: &str) -> Timetable {
    let engine = start_test(file!());
    Timetable::new(
        engine.top(),
        TimetableFile::from_string(yaml).unwrap(),
        platform,
    )
    .unwrap()
}

#[test]
fn bandwidth_graph_finds_widest_path() {
    let (_platform, _cfg, graph, mut cache) = build_platform_and_graph();

    assert_eq!(
        graph.cached_widest_path_bandwidth(&mut cache, "pe:pe0", "mem:hbm0"),
        Some(10)
    );
    assert_eq!(
        graph.cached_widest_path_bandwidth(&mut cache, "pe:missing", "mem:hbm0"),
        None
    );
}

#[test]
fn roofline_and_critical_path_are_deterministic_for_a_chain() {
    let (platform, _cfg, graph, mut cache) = build_platform_and_graph();
    let timetable = build_timetable(&platform, CHAIN_TIMETABLE_YAML);
    let compute_nodes = timetable.compute_node_analyses().unwrap();

    let rooflines = compute_node_rooflines(&platform, &compute_nodes, &graph, &mut cache).unwrap();

    assert_eq!(rooflines.len(), 2);
    for node in &rooflines {
        assert_eq!(node.analysis.total_bytes(), 30);
        assert_eq!(node.analysis.flops, 10);
        assert_eq!(format_machine_ops(&node.analysis.machine_ops), "add=10");
        assert_eq!(node.memory_ticks, 3.0);
        assert_eq!(node.compute_ticks, 10.0);
        assert_eq!(node.roofline_ticks, 10.0);
    }
    assert_eq!(
        rooflines[1].analysis.predecessor_compute_node_ids,
        vec!["add0".to_string()]
    );

    let critical_path = critical_path_analysis(&rooflines).unwrap();
    assert_eq!(critical_path.total_ticks, 20.0);
    assert_eq!(critical_path.node_indices.len(), 2);
}

#[test]
fn compute_ticks_are_rounded_up() {
    let platform_yaml = PLATFORM_YAML.replace("adds_per_tick: 1.0", "adds_per_tick: 6.0");
    let (platform, _cfg, graph, mut cache) = build_platform_and_graph_from(&platform_yaml);
    let timetable = build_timetable(&platform, CHAIN_TIMETABLE_YAML);
    let compute_nodes = timetable.compute_node_analyses().unwrap();

    let rooflines = compute_node_rooflines(&platform, &compute_nodes, &graph, &mut cache).unwrap();

    assert_eq!(rooflines.len(), 2);
    assert!(rooflines.iter().all(|node| node.compute_ticks == 2.0));
}

#[test]
fn scheduler_keeps_compute_bound_memory_at_base_when_not_oversubscribed() {
    let (platform, _cfg, graph, mut cache) = build_platform_and_graph();
    let timetable = build_timetable(&platform, CHAIN_TIMETABLE_YAML);
    let compute_nodes = timetable.compute_node_analyses().unwrap();
    let rooflines = compute_node_rooflines(&platform, &compute_nodes, &graph, &mut cache).unwrap();

    let scheduled = schedule_pe_activities(&platform, &rooflines, &graph, &mut cache).unwrap();

    assert_eq!(scheduled.activities.len(), 2);
    for activity in &scheduled.activities {
        assert_eq!(activity.base_memory_ticks_by_memory["hbm0"], 3.0);
        assert_eq!(activity.adjusted_memory_ticks_by_memory["hbm0"], 3.0);
    }
    assert_eq!(scheduled.activities[0].end_ticks, 10.0);
    assert_eq!(scheduled.activities[1].end_ticks, 20.0);
}

#[test]
fn scheduler_accounts_for_shared_memory_contention() {
    let fast_compute_platform = PLATFORM_YAML.replace("adds_per_tick: 1.0", "adds_per_tick: 10.0");
    let (platform, _cfg, graph, mut cache) = build_platform_and_graph_from(&fast_compute_platform);
    let timetable = build_timetable(&platform, PARALLEL_TIMETABLE_YAML);
    let compute_nodes = timetable.compute_node_analyses().unwrap();
    let rooflines = compute_node_rooflines(&platform, &compute_nodes, &graph, &mut cache).unwrap();

    assert_eq!(rooflines.len(), 2);
    assert!(rooflines.iter().all(|node| node.compute_ticks == 1.0));
    assert!(rooflines.iter().all(|node| node.memory_ticks == 3.0));
    assert!(rooflines.iter().all(|node| node.roofline_ticks == 3.0));

    let scheduled = schedule_pe_activities(&platform, &rooflines, &graph, &mut cache).unwrap();

    assert_eq!(scheduled.activities.len(), 2);
    assert!(
        scheduled
            .activities
            .iter()
            .all(|activity| activity.start_ticks == 0.0)
    );
    assert!(
        scheduled
            .activities
            .iter()
            .all(|activity| activity.end_ticks == 6.0)
    );

    let hbm = scheduled.memory_analyses.get("hbm0").unwrap();
    assert_eq!(hbm.windows.len(), 1);
    assert_eq!(hbm.windows[0].requested_fraction, 1.0);
}

#[test]
fn aggregate_pe_rooflines_reports_all_pes() {
    let (platform, _cfg, graph, mut cache) = build_platform_and_graph();
    let timetable = build_timetable(&platform, CHAIN_TIMETABLE_YAML);
    let compute_nodes = timetable.compute_node_analyses().unwrap();
    let rooflines = compute_node_rooflines(&platform, &compute_nodes, &graph, &mut cache).unwrap();

    let summaries = aggregate_pe_rooflines(&platform, &rooflines, &graph, &mut cache).unwrap();

    assert_eq!(summaries.len(), 2);
    assert!(summaries.iter().all(|summary| summary.compute_nodes == 1));
    assert!(summaries.iter().all(|summary| summary.total_bytes == 30));
    assert!(
        summaries
            .iter()
            .all(|summary| summary.roofline_ticks == 10.0)
    );
}

#[test]
fn roofline_analyzer_returns_structured_report() {
    let (platform, cfg, _graph, _cache) = build_platform_and_graph();
    let timetable = build_timetable(&platform, CHAIN_TIMETABLE_YAML);
    let analyzer = RooflineAnalyzer::new(&platform, &cfg).unwrap();

    let report = analyzer.analyze(&platform, &timetable).unwrap();

    assert_eq!(report.timetable_stats.num_compute_nodes, 2);
    assert_eq!(report.compute_nodes.len(), 2);
    assert_eq!(report.node_rooflines.len(), 2);
    assert_eq!(report.pe_summaries.len(), 2);
    assert!(report.scheduled_activities.is_some());
    assert_eq!(report.scheduled_runtime_ticks, Some(20.0));
    assert_eq!(report.critical_path.total_ticks, 20.0);
    assert_eq!(report.estimated_best_case_ticks, 20.0);
}

#[test]
fn metric_overlay_contains_visualisation_compatible_per_pe_metrics() {
    let (platform, cfg, _graph, _cache) = build_platform_and_graph();
    let timetable = build_timetable(&platform, CHAIN_TIMETABLE_YAML);
    let analyzer = RooflineAnalyzer::new(&platform, &cfg).unwrap();
    let report = analyzer.analyze(&platform, &timetable).unwrap();
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let overlay = MetricOverlay::from_roofline_report(&clock, &report);

    assert_eq!(overlay.metrics.len(), 5);
    assert_eq!(overlay.metrics["estimated_compute_ns"].unit, "ns");
    assert_eq!(overlay.metrics["estimated_memory_ns"].unit, "ns");
    assert_eq!(overlay.metrics["estimated_scheduled_finish_ns"].unit, "ns");
    assert_eq!(overlay.metrics["estimated_compute_efficiency"].unit, "%");
    assert_eq!(overlay.metrics["estimated_memory_efficiency"].unit, "%");
    assert_eq!(
        overlay.metrics_by_pe["pe0"]["estimated_compute_ns"],
        gwr_timetable::analysis::ticks_to_ns(&clock, 10.0)
    );
    assert_eq!(
        overlay.metrics_by_pe["pe1"]["estimated_scheduled_finish_ns"],
        gwr_timetable::analysis::ticks_to_ns(&clock, 20.0)
    );
    assert_eq!(
        overlay.metrics_by_pe["pe0"]["estimated_compute_efficiency"],
        100.0
    );
    assert_eq!(
        overlay.metrics_by_pe["pe0"]["estimated_memory_efficiency"],
        30.0
    );

    let output = tempfile::NamedTempFile::new().unwrap();
    overlay.write_json(output.path()).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(output.path()).unwrap()).unwrap();
    assert_eq!(
        json["metrics"]["estimated_compute_efficiency"]["label"],
        "Estimated compute efficiency"
    );
    assert!(json["metrics_by_pe"]["pe0"]["estimated_memory_ns"].is_number());
}

#[test]
fn roofline_analyzer_can_skip_scheduled_activity_analysis() {
    let (platform, cfg, _graph, _cache) = build_platform_and_graph();
    let timetable = build_timetable(&platform, CHAIN_TIMETABLE_YAML);
    let analyzer = RooflineAnalyzer::new(&platform, &cfg).unwrap();

    let report = analyzer
        .analyze_with_options(
            &platform,
            &timetable,
            RooflineAnalysisOptions {
                schedule_activities: false,
                ..RooflineAnalysisOptions::default()
            },
        )
        .unwrap();

    assert!(report.scheduled_activities.is_none());
    assert_eq!(report.scheduled_runtime_ticks, None);
    assert_eq!(report.critical_path.total_ticks, 20.0);
    assert_eq!(report.estimated_best_case_ticks, 20.0);

    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let overlay = MetricOverlay::from_roofline_report(&clock, &report);
    assert!(
        !overlay
            .metrics
            .contains_key("estimated_scheduled_finish_ns")
    );
    assert!(!overlay.metrics_by_pe["pe0"].contains_key("estimated_compute_efficiency"));
    assert!(!overlay.metrics_by_pe["pe0"].contains_key("estimated_memory_efficiency"));
}

#[test]
fn best_case_cache_model_reports_cross_pe_gemm_tensor_sharing() {
    let (platform, cfg, _graph, _cache) =
        build_platform_and_graph_from(SHARED_CACHE_GEMM_PLATFORM_YAML);
    let timetable = build_timetable(&platform, SHARED_CACHE_GEMM_TIMETABLE_YAML);
    let analyzer = RooflineAnalyzer::new(&platform, &cfg).unwrap();

    let worst_report = analyzer
        .analyze_with_options(
            &platform,
            &timetable,
            RooflineAnalysisOptions {
                schedule_activities: false,
                cache_model: CacheModel::Worst,
            },
        )
        .unwrap();
    let best_report = analyzer
        .analyze_with_options(
            &platform,
            &timetable,
            RooflineAnalysisOptions {
                schedule_activities: false,
                cache_model: CacheModel::Best,
            },
        )
        .unwrap();

    assert_eq!(worst_report.cache_sharing.memory_bytes_avoided, 0);
    assert_eq!(best_report.cache_sharing.memory_bytes_avoided, 4);
    assert_eq!(best_report.cache_sharing.original_memory_bytes, 24);
    assert_eq!(best_report.cache_sharing.adjusted_memory_bytes, 20);
    assert_eq!(best_report.cache_sharing.shared_lines.len(), 1);

    let shared_line = &best_report.cache_sharing.shared_lines[0];
    assert_eq!(shared_line.cache_name, "shared");
    assert_eq!(shared_line.memory_name, "hbm0");
    assert_eq!(shared_line.line_addr, 0x2000);
    assert_eq!(shared_line.pes, vec!["pe0".to_string(), "pe1".to_string()]);
    assert_eq!(
        shared_line.node_ids,
        vec!["gemm0".to_string(), "gemm1".to_string()]
    );

    let worst_gemm1 = worst_report
        .node_rooflines
        .iter()
        .find(|node| node.analysis.id == "gemm1")
        .unwrap();
    let best_gemm1 = best_report
        .node_rooflines
        .iter()
        .find(|node| node.analysis.id == "gemm1")
        .unwrap();
    assert_eq!(worst_gemm1.bytes_by_memory["hbm0"], 12);
    assert_eq!(best_gemm1.bytes_by_memory["hbm0"], 8);
    assert!(best_gemm1.memory_ticks < worst_gemm1.memory_ticks);
}

#[test]
fn analysis_rejects_tensor_views_that_span_memories() {
    let (platform, _cfg, _graph, _cache) =
        build_platform_and_graph_from(SPLIT_MEMORY_PLATFORM_YAML);
    let timetable = build_timetable(&platform, SPANNING_TIMETABLE_YAML);

    let err = timetable.compute_node_analyses().unwrap_err();

    assert!(format!("{err}").contains("Tensor view spans memories"));
}

#[test]
fn analysis_rejects_offset_views_outside_the_pe_memory_map() {
    let (platform, _cfg, _graph, _cache) =
        build_platform_and_graph_from(INACCESSIBLE_VIEW_PLATFORM_YAML);
    let timetable = build_timetable(&platform, INACCESSIBLE_VIEW_TIMETABLE_YAML);

    let err = timetable.compute_node_analyses().unwrap_err();

    assert!(format!("{err}").contains("not in the PE memory map"));
}
