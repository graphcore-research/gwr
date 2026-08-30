// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::operators::HasShape;
use gwr_timetable::ComputeTensorDirection;
use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn graph_resolves_tensor_connections_and_implicit_edge_indices() {
    let graph = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 4096, dtype: int8, shape: [2, 4] }
  - id: compute
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views:
      - { offsets: [0, 1], shape: [2, 1] }
    output_views: []
edges:
  - { from: input, to: compute, kind: data }
",
    )
    .unwrap()
    .into_graph()
    .unwrap();

    assert_eq!(graph.nodes()[0].id(), "input");
    assert_eq!(graph.nodes()[1].id(), "compute");
    assert_eq!(graph.nodes()[1].input_edges(), &[Some(0)]);
    assert_eq!(graph.nodes()[0].successors(), &[1]);
    assert_eq!(graph.nodes()[1].predecessors(), &[0]);

    let edge = &graph.edges()[0];
    assert_eq!(edge.from().node(), 0);
    assert_eq!(edge.from().edge_index(), Some(0));
    assert_eq!(edge.to().node(), 1);
    assert_eq!(edge.to().edge_index(), Some(0));

    let connection = edge.tensor_connection().unwrap();
    assert_eq!(connection.tensor_node(), 0);
    assert_eq!(connection.compute_node(), 1);
    assert_eq!(connection.compute_tensor_index(), 0);
    assert_eq!(connection.direction(), ComputeTensorDirection::Input);
    assert_eq!(connection.view().shape().dims(), &[2, 1]);
    assert_eq!(connection.view().offsets(), &[0, 1]);
}
