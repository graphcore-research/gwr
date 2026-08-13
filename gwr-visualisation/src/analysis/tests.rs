// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;

use super::*;
use crate::analysis::memory::summarize_memory;
use crate::analysis::model::{
    OverlayMetricMetadata, TensorPeConsumption, TensorSummary, TensorTrafficAccess,
    TensorTrafficRange,
};
use crate::analysis::tensors::summarize_tensor_traffic;

fn small_timetable() -> TimetableFile {
    TimetableFile::from_file(Path::new("../gwr-timetable/examples/small.yaml")).unwrap()
}

fn tensor_traffic(pe: &str, addr: u64, num_bytes: u64) -> TensorPeConsumption {
    TensorPeConsumption {
        pe: pe.to_string(),
        bytes: num_bytes,
        edge_count: 1,
        by_layer: BTreeMap::new(),
        accesses: vec![TensorTrafficAccess {
            layer: None,
            ranges: vec![TensorTrafficRange { addr, num_bytes }],
        }],
    }
}

#[test]
fn tensor_traffic_summary_saturates_connection_totals() {
    let tensors_by_id = BTreeMap::from([(
        "large".to_string(),
        TensorSummary {
            id: "large".to_string(),
            addr: 0,
            num_bytes: u64::MAX,
            dtype: "int8".to_string(),
            shape: vec![usize::MAX],
            production_by_pe: vec![
                tensor_traffic("pe0", 0, u64::MAX - 1),
                tensor_traffic("pe1", 0, 2),
            ],
            consumption_by_pe: vec![
                tensor_traffic("pe0", 0, u64::MAX - 1),
                tensor_traffic("pe1", 0, 2),
            ],
        },
    )]);

    assert_eq!(
        summarize_tensor_traffic(&tensors_by_id),
        (u64::MAX, u64::MAX)
    );

    let json = serde_json::to_value(&tensors_by_id["large"]).unwrap();
    assert_eq!(json["num_bytes"], u64::MAX.to_string());
    assert_eq!(
        json["consumption_by_pe"][0]["bytes"],
        (u64::MAX - 1).to_string()
    );
}

#[test]
fn memory_summary_saturates_connection_and_memory_totals() {
    let tensors_by_id = BTreeMap::from([(
        "large".to_string(),
        TensorSummary {
            id: "large".to_string(),
            addr: 0,
            num_bytes: u64::MAX,
            dtype: "int8".to_string(),
            shape: vec![usize::MAX],
            production_by_pe: vec![
                tensor_traffic("pe0", 0, u64::MAX - 1),
                tensor_traffic("pe1", 0, 2),
            ],
            consumption_by_pe: vec![
                tensor_traffic("pe0", 0, u64::MAX - 1),
                tensor_traffic("pe1", 0, 2),
            ],
        },
    )]);
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps: []
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 18446744073709551615
",
    )
    .unwrap();

    let summary = summarize_memory(&tensors_by_id, Some(&platform));

    assert_eq!(summary.platform_memories[0].read_bytes, u64::MAX);
    assert_eq!(summary.platform_memories[0].write_bytes, u64::MAX);
    assert_eq!(summary.total_memory_read_bytes, u64::MAX);
    assert_eq!(summary.total_memory_write_bytes, u64::MAX);

    let json = serde_json::to_value(summary).unwrap();
    assert_eq!(json["total_memory_read_bytes"], u64::MAX.to_string());
    assert_eq!(
        json["platform_memories"][0]["read_bytes"],
        u64::MAX.to_string()
    );
}

#[test]
fn memory_summary_counts_tensor_ending_at_final_physical_byte() {
    let tensors_by_id = BTreeMap::from([(
        "top_exclusive".to_string(),
        TensorSummary {
            id: "top_exclusive".to_string(),
            addr: u64::MAX - 1,
            num_bytes: 1,
            dtype: "int8".to_string(),
            shape: vec![1],
            production_by_pe: vec![tensor_traffic("pe0", u64::MAX - 1, 1)],
            consumption_by_pe: vec![tensor_traffic("pe0", u64::MAX - 1, 1)],
        },
    )]);
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps: []
memories:
  - name: top
    kind: hbm
    base_address: 18446744073709551614
    capacity_bytes: 1
",
    )
    .unwrap();

    let summary = summarize_memory(&tensors_by_id, Some(&platform));
    let memory = &summary.platform_memories[0];

    assert_eq!(summary.min_addr, Some(u64::MAX - 1));
    assert_eq!(summary.max_addr, Some(u64::MAX));
    assert_eq!(memory.allocated_bytes, 1);
    assert_eq!(memory.read_bytes, 1);
    assert_eq!(memory.write_bytes, 1);
    assert_eq!(memory.tensors, ["top_exclusive"]);
    assert_eq!(summary.total_memory_read_bytes, 1);
    assert_eq!(summary.total_memory_write_bytes, 1);
}

#[test]
fn summarizes_small_timetable_by_pe() {
    let timetable = small_timetable();
    let data = summarize(&timetable, Path::new("small.yaml"), None, None);

    assert_eq!(data.summary.compute_nodes, 3);
    assert_eq!(data.summary.total_machine_ops, 22_579_200);
    assert_eq!(data.summary.tensor_nodes, 6);
    assert_eq!(data.summary.data_edges, 9);
    assert_eq!(data.summary.active_pes, 3);
    assert_eq!(data.tensors.len(), 6);
    assert_eq!(data.summary.total_tensor_read_bytes, 1_204_224);
    assert_eq!(data.summary.total_tensor_write_bytes, 802_816);
    assert_eq!(data.layers.len(), 2);
    assert_eq!(data.layers[0].name, "layer 1");
    assert_eq!(data.layers[0].compute_nodes, 2);
    assert_eq!(data.layers[0].machine_ops.total, 150_528);
    assert_eq!(data.layers[0].tensor_read_bytes, 802_816);
    assert_eq!(data.layers[0].tensor_write_bytes, 602_112);
    assert_eq!(data.layers[0].pes[0].name, "pe_0_0");
    assert_eq!(data.layers[0].pes[0].by_op["add"], 1);
    assert_eq!(data.layers[0].pes[0].tensor_count, 3);
    let pe_0_0 = data.pes.iter().find(|pe| pe.name == "pe_0_0").unwrap();
    assert_eq!(pe_0_0.tensor_read_bytes, 401_408);
    assert_eq!(pe_0_0.tensor_write_bytes, 401_408);
    assert_eq!(pe_0_0.machine_ops.total, 100_352);
    assert_eq!(pe_0_0.machine_ops.adds, 100_352);
    let pe_0_1 = data.pes.iter().find(|pe| pe.name == "pe_0_1").unwrap();
    assert_eq!(data.layers[1].tensor_read_bytes, 401_408);
    assert_eq!(pe_0_1.tensor_read_bytes, 401_408);
    assert_eq!(pe_0_1.tensor_write_bytes, 200_704);
    let pe_1_0 = data.pes.iter().find(|pe| pe.name == "pe_1_0").unwrap();
    assert_eq!(pe_1_0.machine_ops.total, 22_428_672);
    assert_eq!(pe_1_0.machine_ops.adds, 11_189_248);
    assert_eq!(pe_1_0.machine_ops.muls, 11_239_424);
    assert!(data.ops.contains(&"add".to_string()));
    assert!(data.ops.contains(&"gemm".to_string()));
    assert_eq!(
        data.pes
            .iter()
            .find(|pe| pe.name == "pe_0_0")
            .unwrap()
            .total_nodes,
        1
    );

    let tensor = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "tensor_0_0_to_0_1")
        .unwrap();
    assert_eq!(tensor.dtype, "fp32");
    assert_eq!(tensor.shape, vec![2, 224, 224]);
    assert_eq!(tensor.production_by_pe.len(), 1);
    assert_eq!(tensor.production_by_pe[0].pe, "pe_0_0");
    assert_eq!(tensor.production_by_pe[0].bytes, 401_408);
    assert_eq!(
        tensor.production_by_pe[0].by_layer["layer 1"].bytes,
        401_408
    );
    assert_eq!(tensor.consumption_by_pe.len(), 1);
    assert_eq!(tensor.consumption_by_pe[0].pe, "pe_0_1");
    assert_eq!(tensor.consumption_by_pe[0].bytes, 401_408);
    assert_eq!(tensor.consumption_by_pe[0].edge_count, 2);
    assert_eq!(
        tensor.consumption_by_pe[0].by_layer["layer 1"].edge_count,
        2
    );
    assert_eq!(
        tensor.consumption_by_pe[0].by_layer["layer 1"].bytes,
        401_408
    );
}

#[test]
fn joins_platform_pes_and_config() {
    let timetable = small_timetable();
    let platform: PlatformConfig = serde_yaml::from_str(
        &fs::read_to_string("../gwr-platform/examples/platform_4x4.yaml").unwrap(),
    )
    .unwrap();
    let data = summarize(
        &timetable,
        Path::new("small.yaml"),
        Some((&platform, Path::new("platform_4x4.yaml"))),
        None,
    );

    assert_eq!(data.platform.as_ref().unwrap().processing_elements, 15);
    let pe_0_2 = data.pes.iter().find(|pe| pe.name == "pe_0_2").unwrap();
    assert!(pe_0_2.present_in_platform);
    assert!(!pe_0_2.present_in_timetable);
    assert_eq!(
        pe_0_2.platform_config.as_ref().unwrap().num_active_requests,
        Some(8)
    );
}

#[test]
fn places_arbitrarily_named_pes_from_fabric_connections() {
    let timetable = small_timetable();
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps:
  - name: default
    devices: []
fabrics:
  - name: fabric0
    kind: functional
    columns: 4
    rows: 3
processing_elements:
  - name: worker_a
    memory_map: default
    config: {}
  - name: worker_b
    memory_map: default
    config: {}
  - name: worker_c
    memory_map: default
    config: {}
caches:
  - name: cache0
    config: {}
connections:
  - connect: [pe.worker_a, 'fabric.fabric0@(1,2)']
  - connect: [pe.worker_b, 'fabric.fabric0@(3,0)']
  - connect: [pe.worker_c, cache.cache0.dev]
  - connect: [cache.cache0.mem, 'fabric.fabric0@(2,1)']
",
    )
    .unwrap();
    let data = summarize(
        &timetable,
        Path::new("small.yaml"),
        Some((&platform, Path::new("platform.yaml"))),
        None,
    );

    let worker_a = data.pes.iter().find(|pe| pe.name == "worker_a").unwrap();
    let worker_b = data.pes.iter().find(|pe| pe.name == "worker_b").unwrap();
    let worker_c = data.pes.iter().find(|pe| pe.name == "worker_c").unwrap();
    assert_eq!((worker_a.row, worker_a.col), (2, 1));
    assert_eq!((worker_b.row, worker_b.col), (0, 3));
    assert_eq!((worker_c.row, worker_c.col), (1, 2));
}

#[test]
fn joins_platform_memories_and_tensor_allocations() {
    let timetable = small_timetable();
    let platform: PlatformConfig = serde_yaml::from_str(
        &fs::read_to_string("../gwr-platform/examples/platform_4x4_4xhbm.yaml").unwrap(),
    )
    .unwrap();
    let data = summarize(
        &timetable,
        Path::new("small.yaml"),
        Some((&platform, Path::new("platform_4x4_4xhbm.yaml"))),
        None,
    );

    let platform = data.platform.as_ref().unwrap();
    assert_eq!(platform.rows, 4);
    assert_eq!(platform.cols, 4);
    assert_eq!(data.memory.platform_memories.len(), 4);
    assert_eq!(data.memory.total_memory_read_bytes, 1_204_224);
    assert_eq!(data.memory.total_memory_write_bytes, 802_816);
    let hbm0 = data
        .memory
        .platform_memories
        .iter()
        .find(|memory| memory.name == "hbm0")
        .unwrap();
    let hbm1 = data
        .memory
        .platform_memories
        .iter()
        .find(|memory| memory.name == "hbm1")
        .unwrap();
    assert_eq!(hbm0.allocated_bytes, 401_408);
    assert_eq!(hbm0.read_bytes, 401_408);
    assert_eq!(hbm0.write_bytes, 0);
    assert_eq!(hbm0.tensor_count, 2);
    assert_eq!(hbm1.allocated_bytes, 401_408);
    assert_eq!(hbm1.read_bytes, 401_408);
    assert_eq!(hbm1.write_bytes, 401_408);
    assert_eq!(hbm1.tensor_count, 1);
    assert!(hbm1.tensors.contains(&"tensor_0_0_to_0_1".to_string()));

    let json = serde_json::to_value(&data).unwrap();
    let hbm0_json = json["memory"]["platform_memories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|memory| memory["name"] == "hbm0")
        .unwrap();
    assert_eq!(hbm0_json["base_addr"], "4294967296");
    assert!(hbm0_json["capacity_bytes"].is_string());
}

#[test]
fn memory_summary_unions_overlapping_tensor_allocations() {
    let tensors_by_id = BTreeMap::from([
        (
            "first".to_string(),
            TensorSummary {
                id: "first".to_string(),
                addr: 0,
                num_bytes: 8,
                dtype: "int8".to_string(),
                shape: vec![8],
                production_by_pe: Vec::new(),
                consumption_by_pe: Vec::new(),
            },
        ),
        (
            "alias".to_string(),
            TensorSummary {
                id: "alias".to_string(),
                addr: 4,
                num_bytes: 8,
                dtype: "int8".to_string(),
                shape: vec![8],
                production_by_pe: Vec::new(),
                consumption_by_pe: Vec::new(),
            },
        ),
    ]);
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps: []
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 16
",
    )
    .unwrap();

    let summary = summarize_memory(&tensors_by_id, Some(&platform));
    let memory = &summary.platform_memories[0];

    assert_eq!(memory.allocated_bytes, 12);
    assert_eq!(memory.tensor_count, 2);
    assert_eq!(memory.read_bytes, 0);
    assert_eq!(memory.write_bytes, 0);
}

#[test]
fn attributes_tensor_view_traffic_to_its_exact_memory_range() {
    let timetable = TimetableFile::from_string(
        r"
nodes:
  - id: source
    kind: tensor
    config: { addr: 0, dtype: int8, shape: [8] }
  - id: consumer
    kind: compute
    op:
      custom:
        name: consumer
        machine_ops: {}
    pe: pe0
    input_views:
      - offsets: [4]
        shape: [4]
    output_views: []
edges:
  - { from: source, to: consumer, kind: data }
",
    )
    .unwrap();
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps: []
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 4
  - name: hbm1
    kind: hbm
    base_address: 4
    capacity_bytes: 4
",
    )
    .unwrap();

    let data = summarize(
        &timetable,
        Path::new("view-traffic.yaml"),
        Some((&platform, Path::new("platform.yaml"))),
        None,
    );
    let source = &data.tensors[0];

    assert_eq!(source.consumption_by_pe[0].bytes, 4);
    assert_eq!(source.consumption_by_pe[0].accesses.len(), 1);
    assert_eq!(source.consumption_by_pe[0].accesses[0].ranges[0].addr, 4);
    assert_eq!(
        source.consumption_by_pe[0].accesses[0].ranges[0].num_bytes,
        4
    );
    assert_eq!(data.memory.platform_memories[0].read_bytes, 0);
    assert_eq!(data.memory.platform_memories[1].read_bytes, 4);
    assert_eq!(data.memory.total_memory_read_bytes, 4);

    let json = serde_json::to_value(data).unwrap();
    let access = &json["tensors"][0]["consumption_by_pe"][0]["accesses"][0];
    assert_eq!(access["ranges"][0]["addr"], "4");
    assert_eq!(access["ranges"][0]["num_bytes"], "4");
}

#[test]
fn attributes_each_strided_tensor_view_range_to_memory() {
    let timetable = TimetableFile::from_string(
        r"
nodes:
  - id: source
    kind: tensor
    config: { addr: 0, dtype: int8, shape: [4, 4] }
  - id: consumer
    kind: compute
    op:
      custom:
        name: consumer
        machine_ops: {}
    pe: pe0
    input_views:
      - offsets: [1, 1]
        shape: [3, 1]
    output_views: []
edges:
  - { from: source, to: consumer, kind: data }
",
    )
    .unwrap();
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps: []
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 8
  - name: hbm1
    kind: hbm
    base_address: 8
    capacity_bytes: 8
",
    )
    .unwrap();

    let data = summarize(
        &timetable,
        Path::new("strided-view-traffic.yaml"),
        Some((&platform, Path::new("platform.yaml"))),
        None,
    );
    let consumption = &data.tensors[0].consumption_by_pe[0];

    assert_eq!(consumption.bytes, 3);
    assert_eq!(consumption.edge_count, 1);
    assert_eq!(consumption.accesses.len(), 1);
    assert_eq!(
        consumption.accesses[0]
            .ranges
            .iter()
            .map(|range| (range.addr, range.num_bytes))
            .collect::<Vec<_>>(),
        vec![(5, 1), (9, 1), (13, 1)]
    );
    assert_eq!(data.memory.platform_memories[0].read_bytes, 1);
    assert_eq!(data.memory.platform_memories[1].read_bytes, 2);
    assert_eq!(data.memory.total_memory_read_bytes, 3);

    let json = serde_json::to_value(data).unwrap();
    assert_eq!(
        json["tensors"][0]["consumption_by_pe"][0]["accesses"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        json["tensors"][0]["consumption_by_pe"][0]["accesses"][0]["ranges"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn serializes_address_ranges_as_strings() {
    let timetable = TimetableFile::from_string(
        r"
nodes:
  - id: high_tensor
    kind: tensor
    config:
      addr: 9007199254740993
      dtype: fp32
      shape: [1]
edges: []
",
    )
    .unwrap();
    let data = summarize(&timetable, Path::new("high-address.yaml"), None, None);

    let json = serde_json::to_value(data).unwrap();
    assert_eq!(json["tensors"][0]["addr"], "9007199254740993");
    assert_eq!(json["tensors"][0]["num_bytes"], "4");
    assert_eq!(json["memory"]["min_addr"], "9007199254740993");
    assert_eq!(json["memory"]["max_addr"], "9007199254740997");
}

#[test]
fn serializes_top_memory_range_endpoint() {
    let timetable = TimetableFile::from_string(
        r"
nodes:
  - id: top_exclusive
    kind: tensor
    config:
      addr: 18446744073709551614
      dtype: int8
      shape: [1]
edges: []
",
    )
    .unwrap();
    let data = summarize(&timetable, Path::new("top-exclusive.yaml"), None, None);

    let json = serde_json::to_value(data).unwrap();
    assert_eq!(json["memory"]["min_addr"], "18446744073709551614");
    assert_eq!(json["memory"]["max_addr"], "18446744073709551615");
}

#[test]
fn reports_asymmetric_platform_dimensions() {
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps:
  - name: default
    devices: []
fabrics:
  - name: fabric0
    kind: functional
    columns: 12
    rows: 24
processing_elements:
  - name: worker
    memory_map: default
    config: {}
",
    )
    .unwrap();

    let summary = summarize_platform(&platform);

    assert_eq!(summary.rows, 24);
    assert_eq!(summary.cols, 12);
}

#[test]
fn saturates_platform_dimensions_for_maximum_fallback_coordinates() {
    let platform: PlatformConfig = serde_yaml::from_str(&format!(
        r"
memory_maps:
  - name: default
    devices: []
processing_elements:
  - name: pe_{}_0
    memory_map: default
    config: {{}}
",
        usize::MAX
    ))
    .unwrap();

    let summary = summarize_platform(&platform);

    assert_eq!(summary.rows, 1);
    assert_eq!(summary.cols, usize::MAX);
}

#[test]
fn applies_effective_defaults_to_platform_pe_config() {
    let timetable = small_timetable();
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps:
  - name: default
    devices: []
processing_elements:
  - name: pe_0_0
    memory_map: default
    config: {}
",
    )
    .unwrap();

    let data = summarize(
        &timetable,
        Path::new("small.yaml"),
        Some((&platform, Path::new("platform.yaml"))),
        None,
    );
    let config = data
        .pes
        .iter()
        .find(|pe| pe.name == "pe_0_0")
        .unwrap()
        .platform_config
        .as_ref()
        .unwrap();

    assert_eq!(config.num_active_requests, Some(8));
    assert_eq!(config.lsu_access_bytes, Some(32));
    assert_eq!(config.overhead_size_bytes, Some(8));
    assert_eq!(config.sram_bytes, Some(1024 * 1024));
    assert_eq!(config.adds_per_tick, Some(16.0));
    assert_eq!(config.muls_per_tick, Some(4.0));
    assert_eq!(config.compares_per_tick, Some(16.0));
}

#[test]
fn merges_overlay_and_warns_for_unknown_pe() {
    let timetable = small_timetable();
    let overlay = OverlayInput {
        metrics: BTreeMap::from([(
            "cycles".to_string(),
            OverlayMetricMetadata {
                label: Some("Cycles".to_string()),
                unit: Some("cycles".to_string()),
            },
        )]),
        metrics_by_pe: BTreeMap::from([
            (
                "pe_0_0".to_string(),
                BTreeMap::from([("cycles".to_string(), 10.0)]),
            ),
            (
                "pe_9_9".to_string(),
                BTreeMap::from([("cycles".to_string(), 99.0)]),
            ),
        ]),
    };
    let data = summarize(
        &timetable,
        Path::new("small.yaml"),
        None,
        Some((&overlay, Path::new("overlay.json"))),
    );

    let pe_0_0 = data.pes.iter().find(|pe| pe.name == "pe_0_0").unwrap();
    let pe_0_1 = data.pes.iter().find(|pe| pe.name == "pe_0_1").unwrap();
    assert_eq!(pe_0_0.overlays["cycles"], 10.0);
    assert!(pe_0_1.overlays.is_empty());
    assert_eq!(
        data.overlay_metrics["cycles"].label.as_deref(),
        Some("Cycles")
    );
    assert_eq!(
        data.overlay_metrics["cycles"].unit.as_deref(),
        Some("cycles")
    );
    assert_eq!(
        data.warnings,
        vec!["Overlay references unknown PE 'pe_9_9'"]
    );
}

#[test]
fn keeps_unassigned_nodes_separate_from_a_real_unassigned_pe() {
    let timetable = TimetableFile::from_string(
        r"
nodes:
  - id: real_input
    kind: tensor
    config: { addr: 0, dtype: int8, shape: [1] }
  - id: real_compute
    kind: compute
    op: add
    pe: unassigned
    input_views: [null]
    output_views: [null]
  - id: real_output
    kind: tensor
    config: { addr: 1, dtype: int8, shape: [1] }
  - id: synthetic_input
    kind: tensor
    config: { addr: 2, dtype: int8, shape: [1] }
  - id: synthetic_compute
    kind: compute
    op: add
    input_views: [null]
    output_views: [null]
  - id: synthetic_output
    kind: tensor
    config: { addr: 3, dtype: int8, shape: [1] }
edges:
  - { from: real_input, to: real_compute, kind: data }
  - { from: real_compute, to: real_output, kind: data }
  - { from: synthetic_input, to: synthetic_compute, kind: data }
  - { from: synthetic_compute, to: synthetic_output, kind: data }
",
    )
    .unwrap();
    let platform: PlatformConfig = serde_yaml::from_str(
        r"
memory_maps:
  - name: default
    devices: []
processing_elements:
  - name: unassigned
    memory_map: default
    config: {}
  - name: unassigned_1
    memory_map: default
    config: {}
",
    )
    .unwrap();
    let overlay = OverlayInput {
        metrics: BTreeMap::new(),
        metrics_by_pe: BTreeMap::from([
            (
                "unassigned".to_string(),
                BTreeMap::from([("load".to_string(), 10.0)]),
            ),
            (
                "unassigned_2".to_string(),
                BTreeMap::from([("load".to_string(), 20.0)]),
            ),
        ]),
    };

    let data = summarize(
        &timetable,
        Path::new("unassigned.yaml"),
        Some((&platform, Path::new("platform.yaml"))),
        Some((&overlay, Path::new("overlay.json"))),
    );

    assert_eq!(data.summary.active_pes, 2);
    let real = data.pes.iter().find(|pe| pe.name == "unassigned").unwrap();
    let synthetic = data
        .pes
        .iter()
        .find(|pe| pe.name == "unassigned_3")
        .unwrap();
    assert_eq!(real.total_nodes, 1);
    assert_eq!(synthetic.total_nodes, 1);
    assert_eq!((real.tensor_read_bytes, real.tensor_write_bytes), (1, 1));
    assert_eq!(
        (synthetic.tensor_read_bytes, synthetic.tensor_write_bytes),
        (1, 1)
    );
    assert_eq!(real.overlays["load"], 10.0);
    assert!(synthetic.overlays.is_empty());

    let layer = &data.layers[0];
    assert_eq!(layer.pes.len(), 2);
    assert!(layer.pes.iter().any(|pe| pe.name == "unassigned"));
    assert!(layer.pes.iter().any(|pe| pe.name == "unassigned_3"));
    let real_input = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "real_input")
        .unwrap();
    let synthetic_input = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "synthetic_input")
        .unwrap();
    assert_eq!(real_input.consumption_by_pe[0].pe, "unassigned");
    assert_eq!(synthetic_input.consumption_by_pe[0].pe, "unassigned_3");
}

#[test]
fn uses_single_view_when_edge_index_is_omitted() {
    let yaml = r"
nodes:
  - id: source
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [1, 4]
  - id: writer
    kind: compute
    op: add
    pe: pe_0_0
    input_views:
      - offsets: [0, 0]
        shape: [1, 1]
    output_views:
      - offsets: [0, 0]
        shape: [1, 1]
  - id: target
    kind: tensor
    config:
      addr: 64
      dtype: fp32
      shape: [1, 4]
edges:
  - from: source
    to: writer
    kind: data
  - from: writer
    to: target
    kind: data
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("views.yaml"), None, None);
    let source = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "source")
        .unwrap();
    let target = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "target")
        .unwrap();

    assert_eq!(source.consumption_by_pe[0].bytes, 4);
    assert_eq!(target.production_by_pe[0].bytes, 4);
}

#[test]
fn counts_unaligned_sub_byte_compute_views_as_dispatched_bytes() {
    let yaml = r"
nodes:
  - id: source
    kind: tensor
    config: { addr: 0, dtype: int4, shape: [4] }
  - id: compute
    kind: compute
    op: add
    pe: pe0
    input_views:
      - offsets: [1]
        shape: [2]
    output_views:
      - offsets: [1]
        shape: [2]
  - id: target
    kind: tensor
    config: { addr: 16, dtype: int4, shape: [4] }
edges:
  - { from: source, to: compute, kind: data }
  - { from: compute, to: target, kind: data }
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(
        &timetable,
        Path::new("sub-byte-compute-views.yaml"),
        None,
        None,
    );
    let source = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "source")
        .unwrap();
    let target = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "target")
        .unwrap();

    assert_eq!(data.summary.total_tensor_read_bytes, 2);
    assert_eq!(data.summary.total_tensor_write_bytes, 2);
    assert_eq!(source.consumption_by_pe[0].bytes, 2);
    assert_eq!(target.production_by_pe[0].bytes, 2);
}

#[test]
fn counts_full_tensor_compute_reads() {
    let timetable =
        TimetableFile::from_file(Path::new("../gwr-timetable/examples/cache.yaml")).unwrap();
    let data = summarize(&timetable, Path::new("cache.yaml"), None, None);
    let tensor = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "tensor_A")
        .unwrap();
    let pe0 = data.pes.iter().find(|pe| pe.name == "pe0").unwrap();

    assert_eq!(data.summary.compute_nodes, 1);
    assert_eq!(data.summary.total_tensor_read_bytes, 64);
    assert_eq!(tensor.consumption_by_pe.len(), 1);
    assert_eq!(tensor.consumption_by_pe[0].pe, "pe0");
    assert_eq!(tensor.consumption_by_pe[0].bytes, 64);
    assert_eq!(tensor.consumption_by_pe[0].edge_count, 2);
    assert_eq!(pe0.tensor_read_bytes, 64);
}

#[test]
fn counts_compute_node_views() {
    let yaml = r"
nodes:
  - id: source
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [8]
  - id: reader
    kind: compute
    op: { custom: { machine_ops: {} } }
    pe: pe0
    input_views:
      -
        offsets: [2]
        shape: [3]
    output_views: []
  - id: writer
    kind: compute
    op: { custom: { machine_ops: {} } }
    pe: pe1
    input_views: []
    output_views:
      -
        offsets: [0]
        shape: [2]
  - id: target
    kind: tensor
    config:
      addr: 64
      dtype: fp32
      shape: [8]
edges:
  - from: source
    to: reader
    kind: data
  - from: writer
    to: target
    kind: data
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("compute-views.yaml"), None, None);
    let source = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "source")
        .unwrap();
    let target = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "target")
        .unwrap();
    let pe0 = data.pes.iter().find(|pe| pe.name == "pe0").unwrap();
    let pe1 = data.pes.iter().find(|pe| pe.name == "pe1").unwrap();

    assert_eq!(data.summary.total_tensor_read_bytes, 12);
    assert_eq!(data.summary.total_tensor_write_bytes, 8);
    assert_eq!(source.consumption_by_pe[0].bytes, 12);
    assert_eq!(target.production_by_pe[0].bytes, 8);
    assert_eq!(pe0.tensor_read_bytes, 12);
    assert_eq!(pe1.tensor_write_bytes, 8);
}

#[test]
fn summarizes_resnet_style_names_and_coordinates() {
    let yaml = r"
nodes:
  - id: data
    kind: tensor
    config:
      addr: 0x1_0000_0000
      dtype: fp32
      shape: [1, 3, 224, 224]
  - id: resnetv17_stage3_relu1_fwd_1_part_0
    kind: compute
    op:
      maxpool:
        kernel_shape: [1, 1]
    pe: pe_10_23
    input_views: []
    output_views: []
edges: []
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("resnet.yaml"), None, None);

    assert_eq!(data.summary.compute_nodes, 1);
    assert_eq!(data.layers[0].name, "layer 1");
    assert_eq!(data.ops, vec!["maxpool"]);
    assert_eq!(data.pes[0].col, 10);
    assert_eq!(data.pes[0].row, 23);
}

#[test]
fn keeps_a_chain_fed_by_one_root_tensor_in_one_layer() {
    let yaml = r"
nodes:
  - id: input
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [1]
  - id: first
    kind: compute
    op: add
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: mid
    kind: tensor
    config:
      addr: 4
      dtype: fp32
      shape: [1]
  - id: second
    kind: compute
    op: add
    pe: pe_0_1
    input_views: []
    output_views: []
  - id: output
    kind: tensor
    config:
      addr: 8
      dtype: fp32
      shape: [1]
edges:
  - from: input
    to: first
    kind: data
  - from: first
    to: mid
    kind: data
  - from: mid
    to: second
    kind: data
  - from: second
    to: output
    kind: data
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("layers.yaml"), None, None);

    assert_eq!(
        data.layers
            .iter()
            .map(|layer| layer.name.as_str())
            .collect::<Vec<_>>(),
        ["layer 1"]
    );
    assert_eq!(data.pes[0].by_layer["layer 1"], 1);
    assert_eq!(data.pes[1].by_layer["layer 1"], 1);
}

#[test]
fn derives_layers_from_reverse_ordered_edges() {
    let yaml = r"
nodes:
  - id: input
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [1]
  - id: first
    kind: compute
    op: add
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: middle
    kind: tensor
    config:
      addr: 4
      dtype: fp32
      shape: [1]
  - id: second
    kind: compute
    op: add
    pe: pe_0_1
    input_views: []
    output_views: []
  - id: output
    kind: tensor
    config:
      addr: 8
      dtype: fp32
      shape: [1]
edges:
  - from: second
    to: output
    kind: data
  - from: middle
    to: second
    kind: data
  - from: first
    to: middle
    kind: data
  - from: input
    to: first
    kind: data
";
    let timetable = TimetableFile::from_string(yaml).unwrap();

    assert_eq!(
        compute_graph_layers(&timetable),
        BTreeMap::from([("first".to_string(), 1), ("second".to_string(), 1)])
    );
}

#[test]
fn continues_a_late_disconnected_root_after_the_graph_depth() {
    let yaml = r"
nodes:
  - id: first_root
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [1] }
  - id: first
    kind: compute
    op: gemm
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: intermediate
    kind: tensor
    config: { addr: 4, dtype: fp32, shape: [1] }
  - id: second_root
    kind: tensor
    config: { addr: 8, dtype: fp32, shape: [1] }
  - id: second
    kind: compute
    op: gemm
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: late_root
    kind: tensor
    config: { addr: 12, dtype: fp32, shape: [1] }
  - id: late
    kind: compute
    op: gemm
    pe: pe_0_0
    input_views: []
    output_views: []
edges:
  - { from: first_root, to: first.0, kind: data }
  - { from: first, to: intermediate, kind: data }
  - { from: intermediate, to: second.0, kind: data }
  - { from: second_root, to: second.1, kind: data }
  - { from: late_root, to: late.0, kind: data }
";
    let timetable = TimetableFile::from_string(yaml).unwrap();

    assert_eq!(
        compute_graph_layers(&timetable),
        BTreeMap::from([
            ("first".to_string(), 1),
            ("late".to_string(), 3),
            ("second".to_string(), 2),
        ])
    );
}

#[test]
fn ignores_control_edges_when_deriving_layers() {
    let yaml = r"
nodes:
  - id: first
    kind: compute
    op: add
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: second
    kind: compute
    op: add
    pe: pe_0_1
    input_views: []
    output_views: []
edges:
  - from: first
    to: second
    kind: control
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("control.yaml"), None, None);

    assert_eq!(data.layers.len(), 1);
    assert_eq!(data.layers[0].name, "layer 1");
    assert_eq!(data.layers[0].compute_nodes, 2);
    assert_eq!(data.pes[0].by_layer["layer 1"], 1);
    assert_eq!(data.pes[1].by_layer["layer 1"], 1);
}

#[test]
fn ignores_control_edges_for_tensor_traffic() {
    let yaml = r"
nodes:
  - id: input
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [1]
  - id: compute
    kind: compute
    op: add
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: output
    kind: tensor
    config:
      addr: 4
      dtype: fp32
      shape: [1]
edges:
  - from: input
    to: compute
    kind: control
  - from: compute
    to: output
    kind: control
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("control-tensors.yaml"), None, None);

    assert_eq!(data.summary.total_tensor_read_bytes, 0);
    assert_eq!(data.summary.total_tensor_write_bytes, 0);
    assert_eq!(data.summary.data_edges, 0);
    assert_eq!(data.layers[0].tensor_read_bytes, 0);
    assert_eq!(data.layers[0].tensor_write_bytes, 0);
    assert_eq!(data.pes[0].tensor_read_bytes, 0);
    assert_eq!(data.pes[0].tensor_write_bytes, 0);
    assert!(data.tensors[0].consumption_by_pe.is_empty());
    assert!(data.tensors[1].production_by_pe.is_empty());
}

#[test]
fn control_edges_do_not_consume_tensor_view_slots() {
    let yaml = r"
nodes:
  - id: control_input
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [4]
  - id: data_input
    kind: tensor
    config:
      addr: 16
      dtype: fp32
      shape: [4]
  - id: compute
    kind: compute
    op: add
    pe: pe_0_0
    input_views:
      - offsets: [0]
        shape: [1]
      - offsets: [1]
        shape: [2]
    output_views: []
edges:
  - from: control_input
    to: compute
    kind: control
  - from: data_input
    to: compute
    kind: data
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("control-slots.yaml"), None, None);
    let control_input = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "control_input")
        .unwrap();
    let data_input = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "data_input")
        .unwrap();

    assert_eq!(data.summary.total_tensor_read_bytes, 4);
    assert_eq!(data.layers[0].tensor_read_bytes, 4);
    assert_eq!(data.pes[0].tensor_read_bytes, 4);
    assert!(control_input.consumption_by_pe.is_empty());
    assert_eq!(data_input.consumption_by_pe[0].bytes, 4);
}

#[test]
fn control_edges_do_not_contribute_machine_ops() {
    let yaml = r"
nodes:
  - id: input_a
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [4]
  - id: input_b
    kind: tensor
    config:
      addr: 16
      dtype: fp32
      shape: [4]
  - id: compute
    kind: compute
    op: add
    pe: pe_0_0
    input_views:
      -
      -
    output_views:
      -
  - id: output
    kind: tensor
    config:
      addr: 32
      dtype: fp32
      shape: [4]
edges:
  - from: input_a
    to: compute
    kind: control
  - from: input_b
    to: compute
    kind: control
  - from: compute
    to: output
    kind: control
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("control-ops.yaml"), None, None);

    assert_eq!(data.summary.total_machine_ops, 0);
    assert_eq!(data.layers[0].machine_ops.total, 0);
    assert_eq!(data.pes[0].machine_ops.total, 0);
}

#[test]
fn counts_tensorless_custom_compute_machine_ops() {
    let yaml = r"
nodes:
  - id: custom
    kind: compute
    op:
      custom:
        name: fft_stage
        machine_ops:
          adds: 10
          muls: 20
          compares: 30
    pe: pe_0_0
    input_views: []
    output_views: []
edges: []
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("custom.yaml"), None, None);

    assert_eq!(data.summary.total_machine_ops, 60);
    assert_eq!(data.layers[0].machine_ops.total, 60);
    assert_eq!(data.pes[0].machine_ops.total, 60);
    assert_eq!(data.pes[0].machine_ops.adds, 10);
    assert_eq!(data.pes[0].machine_ops.muls, 20);
    assert_eq!(data.pes[0].machine_ops.compares, 30);
}

#[test]
fn total_machine_ops_saturates_across_pes() {
    let yaml = r"
nodes:
  - id: custom_a
    kind: compute
    op:
      custom:
        name: fft_stage
        machine_ops:
          adds: 18446744073709551614
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: custom_b
    kind: compute
    op:
      custom:
        name: fft_stage
        machine_ops:
          adds: 2
    pe: pe_1_0
    input_views: []
    output_views: []
edges: []
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("large-custom.yaml"), None, None);

    assert_eq!(data.summary.total_machine_ops, u64::MAX);
    assert_eq!(data.pes[0].machine_ops.total, u64::MAX - 1);
    assert_eq!(data.pes[1].machine_ops.total, 2);

    let json = serde_json::to_value(data).unwrap();
    assert_eq!(json["summary"]["total_machine_ops"], u64::MAX.to_string());
    assert_eq!(
        json["pes"][0]["machine_ops"]["total"],
        (u64::MAX - 1).to_string()
    );
}

#[test]
fn treats_disconnected_root_fed_operations_as_parallel() {
    let yaml = r"
nodes:
  - id: input
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [1]
  - id: conv_weight
    kind: tensor
    config:
      addr: 4
      dtype: fp32
      shape: [1]
  - id: conv
    kind: compute
    op: gemm
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: conv_out
    kind: tensor
    config:
      addr: 8
      dtype: fp32
      shape: [1]
  - id: relu
    kind: compute
    op: add
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: dense_input_alias
    kind: tensor
    config:
      addr: 12
      dtype: fp32
      shape: [1]
  - id: dense_weight
    kind: tensor
    config:
      addr: 16
      dtype: fp32
      shape: [1]
  - id: dense
    kind: compute
    op: gemm
    pe: pe_0_1
    input_views: []
    output_views: []
  - id: dense_out
    kind: tensor
    config:
      addr: 20
      dtype: fp32
      shape: [1]
  - id: dense_relu
    kind: compute
    op: add
    pe: pe_0_1
    input_views: []
    output_views: []
edges:
  - from: input
    to: conv.0
    kind: data
  - from: conv_weight
    to: conv.1
    kind: data
  - from: conv
    to: conv_out
    kind: data
  - from: conv_out
    to: relu
    kind: data
  - from: dense_input_alias
    to: dense.0
    kind: data
  - from: dense_weight
    to: dense.1
    kind: data
  - from: dense
    to: dense_out
    kind: data
  - from: dense_out
    to: dense_relu
    kind: data
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("learned.yaml"), None, None);

    assert_eq!(
        data.layers
            .iter()
            .map(|layer| layer.name.as_str())
            .collect::<Vec<_>>(),
        ["layer 1"]
    );
    assert_eq!(data.pes[0].by_layer["layer 1"], 2);
    assert_eq!(data.pes[1].by_layer["layer 1"], 2);
}

#[test]
fn keeps_parallel_root_fed_partitions_in_one_layer() {
    let yaml = r"
nodes:
  - id: input_0
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [1]
  - id: weight_0
    kind: tensor
    config:
      addr: 4
      dtype: fp32
      shape: [1]
  - id: conv_0
    kind: compute
    op: gemm
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: input_1
    kind: tensor
    config:
      addr: 8
      dtype: fp32
      shape: [1]
  - id: weight_1
    kind: tensor
    config:
      addr: 12
      dtype: fp32
      shape: [1]
  - id: conv_1
    kind: compute
    op: gemm
    pe: pe_0_1
    input_views: []
    output_views: []
edges:
  - from: input_0
    to: conv_0.0
    kind: data
  - from: weight_0
    to: conv_0.1
    kind: data
  - from: input_1
    to: conv_1.0
    kind: data
  - from: weight_1
    to: conv_1.1
    kind: data
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(&timetable, Path::new("partitioned.yaml"), None, None);

    assert_eq!(data.layers.len(), 1);
    assert_eq!(data.layers[0].name, "layer 1");
    assert_eq!(data.layers[0].compute_nodes, 2);
    assert_eq!(data.pes[0].by_layer["layer 1"], 1);
    assert_eq!(data.pes[1].by_layer["layer 1"], 1);
}

#[test]
fn keeps_interleaved_root_fed_partitions_in_one_layer() {
    let yaml = r"
nodes:
  - id: input
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [1]
  - id: weight_0
    kind: tensor
    config:
      addr: 4
      dtype: fp32
      shape: [1]
  - id: conv_part_0
    kind: compute
    op: gemm
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: part_0_out
    kind: tensor
    config:
      addr: 8
      dtype: fp32
      shape: [1]
  - id: relu_part_0
    kind: compute
    op: add
    pe: pe_0_0
    input_views: []
    output_views: []
  - id: weight_1
    kind: tensor
    config:
      addr: 12
      dtype: fp32
      shape: [1]
  - id: conv_part_1
    kind: compute
    op: gemm
    pe: pe_0_1
    input_views: []
    output_views: []
edges:
  - from: input
    to: conv_part_0.0
    kind: data
  - from: weight_0
    to: conv_part_0.1
    kind: data
  - from: conv_part_0
    to: part_0_out
    kind: data
  - from: part_0_out
    to: relu_part_0
    kind: data
  - from: input
    to: conv_part_1.0
    kind: data
  - from: weight_1
    to: conv_part_1.1
    kind: data
";
    let timetable = TimetableFile::from_string(yaml).unwrap();
    let data = summarize(
        &timetable,
        Path::new("interleaved-partitions.yaml"),
        None,
        None,
    );

    assert_eq!(data.layers.len(), 1);
    assert_eq!(data.layers[0].name, "layer 1");
    assert_eq!(data.layers[0].compute_nodes, 3);
    assert_eq!(data.pes[0].by_layer["layer 1"], 2);
    assert_eq!(data.pes[1].by_layer["layer 1"], 1);
}
