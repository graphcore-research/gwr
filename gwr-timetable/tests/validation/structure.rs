// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::Path;

use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TimetableFile,
};

use super::common::{
    build_timetable, compute, create_default_timetable_file, data_edge, tensor, timetable,
};

#[test]
fn rejects_tensor_to_tensor_data_edges() {
    let timetable_file = timetable(
        vec![
            tensor("tensor0", 0, DataType::Fp32, &[1]),
            tensor("tensor1", 4, DataType::Fp32, &[1]),
        ],
        vec![data_edge("tensor0", "tensor1")],
    );

    let err = timetable_file.validate().unwrap_err();
    assert!(
        format!("{err}")
            .contains("Invalid edge from Tensor node 'tensor0' to Tensor node 'tensor1'")
    );
}

#[test]
fn rejects_compute_to_compute_data_edges() {
    let timetable_file = timetable(
        vec![
            compute("producer", vec![], vec![None]),
            compute("consumer", vec![None], vec![]),
        ],
        vec![data_edge("producer.output0", "consumer.input0")],
    );

    let err = timetable_file.validate().unwrap_err();
    assert!(
        format!("{err}")
            .contains("Invalid data edge from compute node 'producer' to compute node 'consumer'")
    );
}

#[test]
fn tiny_example_is_valid() {
    let timetable_file =
        TimetableFile::from_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/tiny.yaml"))
            .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn timetable_rejects_unknown_top_level_field() {
    let err = TimetableFile::from_string(
        "
nodes: []
edges: []
edegs: []
",
    )
    .unwrap_err();
    assert!(format!("{err}").contains("unknown field `edegs`"));
}

#[test]
fn edge_rejects_unknown_field() {
    let err = TimetableFile::from_string(
        "
nodes: []
edges:
  - from: a
    to: b
    kind: data
    label: typo
",
    )
    .unwrap_err();
    assert!(format!("{err}").contains("unknown field `label`"));
}

#[test]
fn rejects_unknown_processing_element_on_compute_node() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    let NodeSection::Compute { pe, .. } = &mut timetable_file.nodes[2] else {
        panic!("expected compute node");
    };
    *pe = Some("pe1".to_string());

    let err = build_timetable(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Node 'add0' contains invalid PE ID 'pe1'"));
}

#[test]
fn rejects_duplicate_node_ids() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Tensor {
        id: "tensor1".to_string(),
        config: TensorConfigSection {
            addr: 0,
            dtype: DataType::Fp8,
            shape: vec![3, 2, 4],
        },
    });

    let err = build_timetable(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Duplicate Node ID 'tensor1'"));
}

#[test]
fn rejects_unknown_source_node_in_data_edge() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "node2".to_string(),
        to: "add0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = build_timetable(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Edge contains invalid from Node ID 'node2'"));
}

#[test]
fn rejects_unknown_destination_node_in_data_edge() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "add0".to_string(),
        to: "node2".to_string(),
        kind: EdgeKind::Data,
    });

    let err = build_timetable(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Edge contains invalid to Node ID 'node2'"));
}

#[test]
fn rejects_port_on_tensor_endpoint() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "tensor0.invalid".to_string(),
        to: "add0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("Tensor node 'tensor0' endpoint must be a plain node ID"));
}

#[test]
fn rejects_duplicate_named_input_binding() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].to = "add0.A".to_string();
    timetable_file.edges.push(EdgeSection {
        from: "tensor0".to_string(),
        to: "add0.A".to_string(),
        kind: EdgeKind::Data,
    });

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("input port 'A' is connected more than once"));
}

#[test]
fn tensor_fanout_binds_distinct_named_inputs() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[1].from = "tensor0".to_string();

    timetable_file.validate().unwrap();
    let timetable = build_timetable(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);
}

#[test]
fn rejects_unknown_named_input_port() {
    let timetable_file = timetable(
        vec![
            tensor("input", 0, DataType::Fp32, &[1]),
            compute("compute", vec![None], vec![]),
        ],
        vec![data_edge("input", "compute.unknown")],
    );

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("Node 'compute' has unknown input port 'unknown'"));
}

#[test]
fn resolves_many_tensor_fanout_edges_in_source_order() {
    const NUM_CONNECTIONS: usize = 50_000;

    let mut nodes = Vec::with_capacity(NUM_CONNECTIONS + 1);
    nodes.push(tensor("input", 0, DataType::Int8, &[1]));
    let mut edges = Vec::with_capacity(NUM_CONNECTIONS);
    for index in 0..NUM_CONNECTIONS {
        let compute_id = format!("consumer{index}");
        nodes.push(compute(&compute_id, vec![None], vec![]));
        edges.push(data_edge("input", &format!("{compute_id}.input0")));
    }

    let graph = timetable(nodes, edges).into_graph().unwrap();
    for (tensor_index, edge) in graph.edges().iter().enumerate() {
        assert_eq!(edge.from().node(), 0);
        assert_eq!(edge.to().node(), tensor_index + 1);
        assert_eq!(edge.to_port_name(&graph), Some("input0"));
    }
}
