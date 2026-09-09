// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::Path;

use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::operators::OperatorCustom;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;
use gwr_platform::types::PlatformConfig;

use super::{build_report, compute, compute_with_views, data, graph, tensor, view};

fn custom() -> ComputeOp {
    ComputeOp::Custom(OperatorCustom {
        name: None,
        machine_ops: MachineOpCounts::default(),
    })
}

fn report_with_platform(
    graph: &gwr_timetable::TimetableGraph,
    platform: &PlatformConfig,
) -> crate::model::ReportData {
    build_report(
        graph,
        Path::new("timetable.yaml"),
        Some((platform, Path::new("platform.yaml"))),
        None,
    )
    .unwrap()
}

fn platform_with_memories(memories: &str) -> PlatformConfig {
    serde_yaml::from_str(&format!("memory_maps: []\nmemories:\n{memories}")).unwrap()
}

#[test]
fn preserves_the_exclusive_endpoint_after_the_final_physical_byte() {
    let graph = graph(
        vec![tensor("final", u64::MAX, DataType::Int8, &[1])],
        vec![],
    );
    let platform = platform_with_memories(
        "  - name: final\n    kind: hbm\n    base_address: 18446744073709551615\n    config:\n      capacity_bytes: 1\n",
    );

    let data = report_with_platform(&graph, &platform);
    let memory = &data.memory.platform_memories[0];
    assert_eq!(data.memory.min_addr, Some(u64::MAX));
    assert_eq!(data.memory.max_addr, Some(u128::from(u64::MAX) + 1));
    assert_eq!(memory.allocated_bytes, 1);
    assert_eq!(memory.tensors, ["final"]);

    let json = serde_json::to_value(data).unwrap();
    assert_eq!(json["memory"]["max_addr"], "18446744073709551616");
}

#[test]
fn counts_the_union_of_aliased_tensor_allocations() {
    let graph = graph(
        vec![
            tensor("first", 0, DataType::Int8, &[8]),
            tensor("second", 4, DataType::Int8, &[8]),
        ],
        vec![],
    );
    let platform = platform_with_memories(
        "  - name: memory\n    kind: hbm\n    base_address: 0\n    config:\n      capacity_bytes: 12\n",
    );

    let memory = &report_with_platform(&graph, &platform)
        .memory
        .platform_memories[0];
    assert_eq!(memory.allocated_bytes, 12);
    assert_eq!(memory.tensor_count, 2);
    assert_eq!(memory.tensors, ["first", "second"]);
}

#[test]
fn attributes_large_strided_transfers_without_materializing_ranges() {
    let graph = graph(
        vec![
            tensor("source", 0, DataType::Int8, &[100_000_000, 2]),
            compute_with_views(
                "reader",
                custom(),
                Some("pe0"),
                vec![view(&[100_000_000, 1], &[0, 0])],
                vec![],
            ),
        ],
        vec![data("source", "reader")],
    );
    let platform = platform_with_memories(
        "  - name: lower\n    kind: hbm\n    base_address: 0\n    config:\n      capacity_bytes: 100000000\n  - name: upper\n    kind: hbm\n    base_address: 100000000\n    config:\n      capacity_bytes: 100000000\n",
    );

    let data = report_with_platform(&graph, &platform);
    assert_eq!(data.memory.platform_memories[0].read_bytes, 50_000_000);
    assert_eq!(data.memory.platform_memories[1].read_bytes, 50_000_000);
    assert_eq!(data.memory.total_memory_read_bytes, 100_000_000);
}

#[test]
fn attributes_packed_strided_bytes_to_the_memory_that_contains_them() {
    let graph = graph(
        vec![
            tensor("source", 0, DataType::Int4, &[4, 4]),
            compute_with_views(
                "reader",
                custom(),
                Some("pe0"),
                vec![view(&[3, 1], &[1, 1])],
                vec![],
            ),
        ],
        vec![data("source", "reader")],
    );
    let platform = platform_with_memories(
        "  - name: first\n    kind: hbm\n    base_address: 2\n    config:\n      capacity_bytes: 1\n  - name: gap\n    kind: hbm\n    base_address: 3\n    config:\n      capacity_bytes: 1\n  - name: second\n    kind: hbm\n    base_address: 4\n    config:\n      capacity_bytes: 1\n  - name: third\n    kind: hbm\n    base_address: 6\n    config:\n      capacity_bytes: 1\n",
    );

    let data = report_with_platform(&graph, &platform);
    let reads = data
        .memory
        .platform_memories
        .iter()
        .map(|memory| (memory.name.as_str(), memory.read_bytes))
        .collect::<Vec<_>>();
    assert_eq!(
        reads,
        [("first", 1), ("gap", 0), ("second", 1), ("third", 1)]
    );
}

#[test]
fn rejects_report_tensor_traffic_totals_that_overflow() {
    let graph = graph(
        vec![
            tensor("source", 0, DataType::Int8, &[usize::MAX]),
            compute("first", custom(), Some("pe0"), 1, 0),
            compute("second", custom(), Some("pe1"), 1, 0),
        ],
        vec![data("source", "first"), data("source", "second")],
    );

    let error = build_report(&graph, Path::new("overflow.yaml"), None, None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("tensor read byte total overflows"),
        "{error}"
    );
}
