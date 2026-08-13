// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;
use std::vec;

use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
};
use gwr_track::entity::Entity;

fn create_default_timetable_file() -> (Rc<Entity>, Rc<Platform>, TimetableFile) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    (
        engine.top().clone(),
        Rc::new(
            Platform::from_string(
                &engine,
                &clock,
                "
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
    capacity_bytes: 0x1000_0000
",
            )
            .unwrap(),
        ),
        TimetableFile::from_string(
            "
nodes:
  - id: tensor0
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [3, 10, 10]

  - id: tensor1
    kind: tensor
    config:
      addr: 0x1000
      dtype: fp32
      shape: [3, 10, 10]

  - id: add0
    kind: compute
    op: add
    pe: pe0
    input_views:
      -
      -
    output_views:
      -

  - id: tensor2
    kind: tensor
    config:
      addr: 0x2000
      dtype: fp32
      shape: [3, 10, 10]

edges:
  - from: tensor0
    to: add0.0
    kind: data

  - from: tensor1
    to: add0.1
    kind: data

  - from: add0
    to: tensor2
    kind: data
",
        )
        .unwrap(),
    )
}

#[test]
fn timetable_file() {
    let (top, platform, timetable_file) = create_default_timetable_file();
    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
fn control_edges_are_ignored_by_scheduler() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "tensor2".to_string(),
        to: "add0".to_string(),
        kind: EdgeKind::Control,
    });

    let timetable = Timetable::new(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);

    timetable.set_task_completed(2).unwrap();
    assert!(timetable.ready_task_indices("pe0").unwrap().1.is_empty());
}

#[test]
fn control_edges_ignore_data_port_suffixes() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "tensor2.99".to_string(),
        to: "add0.99".to_string(),
        kind: EdgeKind::Control,
    });

    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
fn control_edges_through_tensors_are_ignored_by_scheduler() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Tensor {
        id: "gate".to_string(),
        config: TensorConfigSection {
            addr: 0,
            dtype: DataType::Fp32,
            shape: vec![1],
        },
    });
    timetable_file.edges.extend([
        EdgeSection {
            from: "tensor2".to_string(),
            to: "gate".to_string(),
            kind: EdgeKind::Control,
        },
        EdgeSection {
            from: "gate".to_string(),
            to: "add0".to_string(),
            kind: EdgeKind::Control,
        },
    ]);

    let timetable = Timetable::new(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);

    timetable.set_task_completed(2).unwrap();
    assert!(timetable.ready_task_indices("pe0").unwrap().1.is_empty());
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
fn compute_node_rejects_unknown_view_field() {
    let err = TimetableFile::from_string(
        "
nodes:
  - id: add0
    kind: compute
    op: add
    pe: pe0
    input_views:
      - offsets: [0]
        shape: [4]
        stride: [1]
      -
    output_views:
      -

edges:
",
    )
    .unwrap_err();
    assert!(format!("{err}").contains("unknown field `stride`"));
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
    timetable_file.nodes.push(NodeSection::Compute {
        id: "node2".to_string(),
        op: ComputeOp::Add,
        pe: Some("pe1".to_string()),
        input_views: vec![None, None],
        output_views: vec![None],
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Node 'node2' contains invalid PE ID 'pe1'"));
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

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Duplicate Node ID 'tensor1'"));
}

#[test]
fn compute_input_view_outside_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    let NodeSection::Compute { input_views, .. } = &mut timetable_file.nodes[2] else {
        panic!("expected compute node");
    };
    input_views[0] = Some(TensorViewSection {
        shape: vec![3, 10, 10],
        offsets: vec![1, 1, 1],
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("input view on node 'add0' is out of range in dim 0"));
}

#[test]
fn compute_output_view_outside_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    let NodeSection::Compute { output_views, .. } = &mut timetable_file.nodes[2] else {
        panic!("expected compute node");
    };
    output_views[0] = Some(TensorViewSection {
        shape: vec![3, 10, 100],
        offsets: vec![0, 0, 0],
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("output view on node 'add0' is out of range in dim 2"));
}

#[test]
fn invalid_from_edge_pe() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "node2".to_string(),
        to: "add0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
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

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Edge contains invalid to Node ID 'node2'"));
}
