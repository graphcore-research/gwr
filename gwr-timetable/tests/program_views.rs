// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::processing_element::tensors::HasShape;
use gwr_timetable::timetable_file::TimetableFile;

const INLINE_ADD: &str = r"
nodes:
  - { id: a4, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - { id: b4, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
  - { id: y4, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4] } }
  - id: add4
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
    tiling: { sizes: { i: 2 } }
  - { id: a8, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [8] } }
  - { id: b8, kind: tensor, config: { addr: 0x5000, dtype: fp32, shape: [8] } }
  - { id: y8, kind: tensor, config: { addr: 0x6000, dtype: fp32, shape: [8] } }
  - id: add8
    kind: compute
    pe: pe1
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
edges:
  - { from: a4, to: add4.A, kind: data }
  - { from: b4, to: add4.B, kind: data }
  - { from: add4.Y, to: y4, kind: data }
  - { from: a8, to: add8.A, kind: data }
  - { from: b8, to: add8.B, kind: data }
  - { from: add8.Y, to: y8, kind: data }
";

#[test]
fn omitted_view_maps_cover_every_resolved_program_port() {
    let graph = TimetableFile::from_string(INLINE_ADD)
        .unwrap()
        .into_graph()
        .unwrap();
    for (id, shape) in [("add4", 4), ("add8", 8)] {
        let index = graph
            .nodes()
            .iter()
            .position(|node| node.id() == id)
            .unwrap();
        let views = graph.compute_views(index).unwrap();
        assert_eq!(views.inputs().len(), 2);
        assert_eq!(views.outputs().len(), 1);
        for view in views.inputs().iter().chain(views.outputs()) {
            assert_eq!(view.shape().dims(), &[shape]);
            assert_eq!(view.offsets(), &[0]);
        }
    }
}

#[test]
fn either_view_map_can_be_omitted_independently() {
    let source = INLINE_ADD
        .replace(
            "    tiling: { sizes: { i: 2 } }\n",
            "    tiling: { sizes: { i: 2 } }\n    output_views: { Y: { offsets: [0], shape: [4] } }\n",
        )
        .replace(
            "    pe: pe1\n",
            "    pe: pe1\n    input_views: { A: { offsets: [0], shape: [8] }, B: { offsets: [0], shape: [8] } }\n",
        );
    let graph = TimetableFile::from_string(&source)
        .unwrap()
        .into_graph()
        .unwrap();
    for id in ["add4", "add8"] {
        let index = graph
            .nodes()
            .iter()
            .position(|node| node.id() == id)
            .unwrap();
        let views = graph.compute_views(index).unwrap();
        assert_eq!(views.inputs().len(), 2);
        assert_eq!(views.outputs().len(), 1);
    }
}

#[test]
fn explicitly_empty_view_map_selects_full_views() {
    let source = INLINE_ADD.replace(
        "    tiling: { sizes: { i: 2 } }\n",
        "    tiling: { sizes: { i: 2 } }\n    input_views: {}\n",
    );
    let graph = TimetableFile::from_string(&source)
        .unwrap()
        .into_graph()
        .unwrap();
    let views = graph.compute_views(3).unwrap();
    assert_eq!(views.inputs()[0].shape().dims(), &[4]);
    assert_eq!(views.inputs()[1].shape().dims(), &[4]);
}
