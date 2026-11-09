// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_platform::types::PlatformConfig;
use gwr_timetable::analysis::roofline::RooflineAnalyzer;
use gwr_timetable::timetable_file::TimetableFile;
use serde_json::json;

#[test]
fn dependent_tasks_on_distinct_memories_keep_each_tasks_memory_service_floor() {
    let config:PlatformConfig=yaml_serde::from_str(r#"
memory_maps:
  - {name: mm0, devices: [mem0]}
  - {name: mm1, devices: [mem1]}
operation_classes:
  binary: {operations: [add], arity: 2}
processing_elements:
  - name: pe0
    memory_map: mm0
    config: {compute: {engines: {vector: {capabilities: [{class: binary, ops_per_tick: 4}]}}}}
  - name: pe1
    memory_map: mm1
    config: {compute: {engines: {vector: {capabilities: [{class: binary, ops_per_tick: 4}]}}}}
memories:
  - {name: mem0, kind: hbm, base_address: 0, config: {capacity_bytes: 4096, bw_bytes_per_tick: 32}}
  - {name: mem1, kind: hbm, base_address: 4096, config: {capacity_bytes: 4096, bw_bytes_per_tick: 32}}
connections:
  - {connect: [pe.pe0, mem.mem0]}
  - {connect: [pe.pe1, mem.mem1]}
"#).unwrap();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for i in 0..2 {
        for (name, offset) in [(format!("a{i}"), 0), (format!("y{i}"), 128)] {
            nodes.push(json!({"id":name,"kind":"tensor","config":{"addr":i*4096+offset,"dtype":"int8","shape":[32]}}));
        }
        nodes.push(json!({"id":format!("task{i}"),"kind":"compute","pe":format!("pe{i}"),"program":{
            "inputs":["A"],"outputs":["Y"],"assignments":[{"output":"Y","axes":["i"],"expr":{"read":{"tensor":"A","indices":["i"]}}}]}}));
        edges.push(json!({"from":format!("a{i}"),"to":format!("task{i}"),"kind":"data"}));
        edges.push(json!({"from":format!("task{i}"),"to":format!("y{i}"),"kind":"data"}));
    }
    let estimate = |edges: &Vec<serde_json::Value>| {
        let graph = TimetableFile::from_string(&json!({"nodes":nodes,"edges":edges}).to_string())
            .unwrap()
            .into_graph()
            .unwrap();
        RooflineAnalyzer::analyze_graph(&config, &graph)
            .unwrap()
            .estimated_ticks
    };
    assert_eq!(estimate(&edges), 2.0);
    edges.push(json!({"from":"task0","to":"task1","kind":"dependency"}));
    // Each task needs 32B read + 32B write at 32B/tick, on distinct memories.
    // Dependency prevents these two independent 2-tick service demands
    // overlapping.
    assert_eq!(estimate(&edges), 4.0);
}
