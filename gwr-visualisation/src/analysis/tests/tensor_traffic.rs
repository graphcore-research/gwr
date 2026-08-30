// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::operators::OperatorCustom;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;

use super::{compute, compute_with_views, control, data, graph, report, tensor, view};

fn custom() -> ComputeOp {
    ComputeOp::Custom(OperatorCustom {
        name: None,
        machine_ops: MachineOpCounts::default(),
    })
}

#[test]
fn records_one_strided_transfer_for_each_data_edge() {
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

    let data = report(&graph);
    let traffic = &data.tensors[0].reads_by_pe[0];
    assert_eq!(traffic.bytes, 100_000_000);
    assert_eq!(traffic.edge_count, 1);
    assert_eq!(traffic.transfers.len(), 1);
    let access = &traffic.transfers[0].access;
    assert_eq!(access.first_element, 0);
    assert_eq!(access.elements_per_range, 1);
    assert_eq!(access.num_access_bytes, 100_000_000);
    assert_eq!(access.strides.len(), 1);
    assert_eq!(access.strides[0].count, 100_000_000);
    assert_eq!(access.strides[0].stride_elements, 2);

    let encoded = serde_json::to_string(&traffic.transfers[0]).unwrap();
    assert!(encoded.len() < 300);
    assert!(!encoded.contains("ranges"));
}

#[test]
fn preserves_unaligned_packed_view_geometry() {
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

    let data = report(&graph);
    let access = &data.tensors[0].reads_by_pe[0].transfers[0].access;
    assert_eq!(access.first_element, 5);
    assert_eq!(access.elements_per_range, 1);
    assert_eq!(access.bits_per_element, 4);
    assert_eq!(access.num_access_bytes, 3);
    assert_eq!(access.strides[0].count, 3);
    assert_eq!(access.strides[0].stride_elements, 4);
    assert_eq!(data.summary.total_tensor_read_bytes, 3);
}

#[test]
fn groups_multiple_edges_without_merging_their_transfers() {
    let graph = graph(
        vec![
            tensor("source", 0, DataType::Int8, &[8]),
            compute_with_views(
                "reader",
                custom(),
                Some("pe0"),
                vec![view(&[4], &[0]), view(&[4], &[4])],
                vec![],
            ),
        ],
        vec![data("source", "reader.0"), data("source", "reader.1")],
    );

    let data = report(&graph);
    let traffic = &data.tensors[0].reads_by_pe[0];
    assert_eq!(traffic.bytes, 8);
    assert_eq!(traffic.edge_count, 2);
    assert_eq!(traffic.transfers.len(), 2);
    assert_eq!(traffic.transfers[0].access.first_element, 0);
    assert_eq!(traffic.transfers[1].access.first_element, 4);
}

#[test]
fn omitting_a_tensor_index_uses_the_resolved_view_slot() {
    let graph = graph(
        vec![
            tensor("source", 0, DataType::Int8, &[8]),
            compute_with_views(
                "reader",
                custom(),
                Some("pe0"),
                vec![view(&[3], &[2])],
                vec![],
            ),
        ],
        vec![data("source", "reader")],
    );

    let data = report(&graph);
    let transfer = &data.tensors[0].reads_by_pe[0].transfers[0];
    assert_eq!(transfer.access.first_element, 2);
    assert_eq!(transfer.access.num_access_bytes, 3);
}

#[test]
fn control_edges_do_not_consume_tensor_indices_or_add_traffic() {
    let graph = graph(
        vec![
            tensor("control", 0, DataType::Int8, &[4]),
            tensor("source", 8, DataType::Int8, &[4]),
            compute("reader", custom(), Some("pe0"), 1, 0),
        ],
        vec![control("control", "reader"), data("source", "reader")],
    );

    let data = report(&graph);
    let control = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "control")
        .unwrap();
    let source = data
        .tensors
        .iter()
        .find(|tensor| tensor.id == "source")
        .unwrap();
    assert!(control.reads_by_pe.is_empty());
    assert_eq!(source.reads_by_pe[0].bytes, 4);
    assert_eq!(data.summary.data_edges, 1);
}

#[test]
fn serializes_dimensions_and_strides_as_decimal_strings() {
    let graph = graph(
        vec![
            tensor("source", 0, DataType::Int8, &[4_294_967_296, 2]),
            compute_with_views(
                "reader",
                custom(),
                Some("pe0"),
                vec![view(&[4_294_967_296, 1], &[0, 0])],
                vec![],
            ),
        ],
        vec![data("source", "reader")],
    );

    let json = serde_json::to_value(report(&graph)).unwrap();
    assert_eq!(json["tensors"][0]["shape"][0], "4294967296");
    let access = &json["tensors"][0]["reads_by_pe"][0]["transfers"][0]["access"];
    assert_eq!(access["first_element"], "0");
    assert_eq!(access["strides"][0]["count"], "4294967296");
    assert_eq!(access["strides"][0]["stride_elements"], "2");
}
