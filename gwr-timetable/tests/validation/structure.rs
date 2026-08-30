// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::*;

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
        vec![data_edge("producer", "consumer")],
    );

    let err = timetable_file.validate().unwrap_err();
    assert!(
        format!("{err}")
            .contains("Invalid data edge from compute node 'producer' to compute node 'consumer'")
    );
}

#[test]
fn tiny_example_is_valid() {
    let timetable_file = TimetableFile::from_file(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/tiny.yaml"),
    )
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
fn invalid_node_pe() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    let NodeSection::Compute { pe, .. } = &mut timetable_file.nodes[2] else {
        panic!("expected compute node");
    };
    *pe = Some("pe1".to_string());

    let err = build_timetable(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Node 'add0' contains invalid PE ID 'pe1'"));
}

#[test]
fn duplicate_node_id() {
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
fn invalid_from_edge_pe() {
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
fn invalid_to_edge_pe() {
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
fn invalid_edge_endpoint_syntax() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "tensor0.invalid".to_string(),
        to: "add0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("Unable to parse edge id 'tensor0.invalid'"));
}

#[test]
fn duplicate_explicit_tensor_index_after_implicit_index() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].to = "add0".to_string();
    timetable_file.edges.push(EdgeSection {
        from: "tensor0".to_string(),
        to: "add0.0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("input tensor index 0 is connected more than once"));
}

#[test]
fn sparse_explicit_and_implicit_tensor_indices_are_allowed() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].from = "tensor0.5".to_string();
    timetable_file.edges[1].from = "tensor0".to_string();

    timetable_file.validate().unwrap();
    let timetable = build_timetable(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);
}

#[test]
fn maximum_tensor_index_does_not_materialize_intervening_indices() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].from = format!("tensor0.{}", usize::MAX);

    timetable_file.validate().unwrap();
    build_timetable(&top, timetable_file, &platform).unwrap();
}

#[test]
fn duplicate_sparse_tensor_indices_are_rejected() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].from = "tensor0.5".to_string();
    timetable_file.edges[1].from = "tensor0.5".to_string();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("output tensor index 5 is connected more than once"));
}

#[test]
fn explicit_tensor_index_is_checked_before_tracking_occupancy() {
    let timetable_file = timetable(
        vec![
            tensor("input", 0, DataType::Fp32, &[1]),
            compute("compute", vec![None], vec![]),
        ],
        vec![data_edge("input", "compute.1000000000")],
    );

    let err = timetable_file.validate().unwrap_err();
    assert!(
        format!("{err}").contains("Node 'compute' input tensor index 1000000000 is out of range")
    );
}

#[test]
fn resolves_many_implicit_tensor_indices_in_source_order() {
    const NUM_CONNECTIONS: usize = 50_000;

    let mut nodes = Vec::with_capacity(NUM_CONNECTIONS + 1);
    nodes.push(tensor("input", 0, DataType::Int8, &[1]));
    let mut edges = Vec::with_capacity(NUM_CONNECTIONS);
    for index in 0..NUM_CONNECTIONS {
        let compute_id = format!("consumer{index}");
        nodes.push(compute(&compute_id, vec![None], vec![]));
        edges.push(data_edge("input", &compute_id));
    }

    let graph = timetable(nodes, edges).into_graph().unwrap();
    for (tensor_index, edge) in graph.edges().iter().enumerate() {
        assert_eq!(edge.from().edge_index(), Some(tensor_index));
    }
}
