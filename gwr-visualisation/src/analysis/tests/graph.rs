// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::operators::OperatorCustom;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;

use super::{compute, control, data, graph, report, tensor};

fn custom() -> ComputeOp {
    ComputeOp::Custom(OperatorCustom {
        name: None,
        machine_ops: MachineOpCounts::default(),
    })
}

#[test]
fn keeps_a_chain_from_one_root_tensor_in_one_layer() {
    let graph = graph(
        vec![
            tensor("root", 0, DataType::Int8, &[1]),
            compute("first", custom(), Some("pe0"), 1, 1),
            tensor("middle", 1, DataType::Int8, &[1]),
            compute("second", custom(), Some("pe1"), 1, 0),
        ],
        vec![
            data("root", "first"),
            data("first", "middle"),
            data("middle", "second"),
        ],
    );

    let data = report(&graph);
    assert_eq!(data.layers.len(), 1);
    assert_eq!(data.layers[0].name, "layer 1");
    assert_eq!(data.layers[0].compute_nodes, 2);
}

#[test]
fn derives_layers_independently_of_edge_order() {
    let nodes = vec![
        tensor("root", 0, DataType::Int8, &[1]),
        compute("first", custom(), Some("pe0"), 1, 1),
        tensor("middle", 1, DataType::Int8, &[1]),
        compute("second", custom(), Some("pe1"), 1, 0),
    ];
    let forward = graph(
        nodes.clone(),
        vec![
            data("root", "first"),
            data("first", "middle"),
            data("middle", "second"),
        ],
    );
    let reverse = graph(
        nodes,
        vec![
            data("middle", "second"),
            data("first", "middle"),
            data("root", "first"),
        ],
    );

    assert_eq!(
        report(&forward)
            .layers
            .iter()
            .map(|layer| (&layer.name, layer.compute_nodes))
            .collect::<Vec<_>>(),
        report(&reverse)
            .layers
            .iter()
            .map(|layer| (&layer.name, layer.compute_nodes))
            .collect::<Vec<_>>()
    );
}

#[test]
fn starts_a_late_disconnected_root_after_the_existing_depth() {
    let graph = graph(
        vec![
            tensor("root_a", 0, DataType::Int8, &[1]),
            compute("first", custom(), Some("pe0"), 1, 1),
            tensor("middle", 1, DataType::Int8, &[1]),
            tensor("root_b", 2, DataType::Int8, &[1]),
            compute("second", custom(), Some("pe1"), 2, 0),
            tensor("late_root", 3, DataType::Int8, &[1]),
            compute("late", custom(), Some("pe2"), 1, 0),
        ],
        vec![
            data("root_a", "first"),
            data("first", "middle"),
            data("middle", "second.0"),
            data("root_b", "second.1"),
            data("late_root", "late"),
        ],
    );

    let data = report(&graph);
    assert_eq!(
        data.layers
            .iter()
            .map(|layer| (layer.name.as_str(), layer.compute_nodes))
            .collect::<Vec<_>>(),
        [("layer 1", 1), ("layer 2", 1), ("layer 3", 1)]
    );
}

#[test]
fn keeps_early_disconnected_roots_parallel() {
    let graph = graph(
        vec![
            tensor("first_root", 0, DataType::Int8, &[1]),
            tensor("second_root", 1, DataType::Int8, &[1]),
            compute("first", custom(), Some("pe0"), 1, 0),
            compute("second", custom(), Some("pe1"), 1, 0),
        ],
        vec![data("first_root", "first"), data("second_root", "second")],
    );

    let data = report(&graph);
    assert_eq!(data.layers.len(), 1);
    assert_eq!(data.layers[0].compute_nodes, 2);
}

#[test]
fn ignores_control_edges_when_calculating_layers() {
    let graph = graph(
        vec![
            tensor("first_root", 0, DataType::Int8, &[1]),
            tensor("second_root", 1, DataType::Int8, &[1]),
            compute("first", custom(), Some("pe0"), 1, 0),
            compute("second", custom(), Some("pe1"), 1, 0),
        ],
        vec![
            data("first_root", "first"),
            data("second_root", "second"),
            control("first", "second"),
        ],
    );

    let data = report(&graph);
    assert_eq!(data.layers.len(), 1);
    assert_eq!(data.layers[0].compute_nodes, 2);
}
