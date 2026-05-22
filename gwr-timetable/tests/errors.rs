// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;
use std::vec;

use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::MemoryOp;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, MemoryConfigSection, NodeSection, TensorConfigSection,
    TensorViewSection, TimetableFile,
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

  - id: load0
    kind: memory
    op: load
    pe: pe0
    config:
      view:
        shape: [1, 10, 10]
        offsets: [0, 0, 0]

  - id: load1
    kind: memory
    op: load
    pe: pe0
    config:
      view:
        shape: [1, 10, 10]
        offsets: [1, 0, 0]

edges:
  - from: tensor0
    to: load0
    kind: data

  - from: tensor0
    to: load1
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

// Node errors

#[test]
fn invalid_node_pe() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "node2".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe1".to_string()),
        config: MemoryConfigSection { view: None },
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Node 'node2' contains invalid PE ID 'pe1'"));
}

#[test]
fn duplicate_node_id() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "load1".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection { view: None },
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Duplicate Node ID 'load1'"));
}

#[test]
fn load_not_connected_to_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "node2".to_string(),
        op: MemoryOp::Load,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection { view: None },
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("0 edges connect into Load node"));
}

#[test]
fn store_not_connected_to_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "node2".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection { view: None },
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("0 edges connect from Store node"));
}

#[test]
fn load_outside_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "node2".to_string(),
        op: MemoryOp::Load,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection {
            view: Some(TensorViewSection {
                shape: vec![3, 10, 10],
                offsets: vec![1, 1, 1],
            }),
        },
    });
    timetable_file.edges.push(EdgeSection {
        from: "tensor0".to_string(),
        to: "node2".to_string(),
        kind: EdgeKind::Data,
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Load view on node 'node2' is out of range in dim 0"));
}

#[test]
fn store_outside_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "store0".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection {
            view: Some(TensorViewSection {
                shape: vec![3, 10, 100],
                offsets: vec![0, 0, 0],
            }),
        },
    });
    timetable_file.nodes.push(NodeSection::Tensor {
        id: "tensor1".to_string(),
        config: TensorConfigSection {
            addr: 0,
            dtype: DataType::Fp8,
            shape: vec![3, 2, 4],
        },
    });
    timetable_file.edges.push(EdgeSection {
        from: "load0".to_string(),
        to: "store0".to_string(),
        kind: EdgeKind::Data,
    });
    timetable_file.edges.push(EdgeSection {
        from: "store0".to_string(),
        to: "tensor1".to_string(),
        kind: EdgeKind::Data,
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Store view on node 'store0' is out of range in dim 1"));
}

// Edge errors

#[test]
fn invalid_from_edge_pe() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "node2".to_string(),
        to: "load0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Edge contains invalid from Node ID 'node2'"));
}

#[test]
fn invalid_to_edge_pe() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "load0".to_string(),
        to: "node2".to_string(),
        kind: EdgeKind::Data,
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Edge contains invalid to Node ID 'node2'"));
}

#[test]
fn graph_cycle() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "load0".to_string(),
        to: "tensor0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = Timetable::new(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains("Timetable graph contains a cycle"));
}

#[test]
fn memory_op_too_big() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Rc::new(
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
      sram_bytes: 128

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 0x1000_0000

connections:
  - connect:
    - pe.pe0
    - mem.hbm0
",
        )
        .unwrap(),
    );
    let timetable_file = TimetableFile::from_string(
        "
nodes:
  - id: tensor0
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [2, 16, 16]

  - id: load0
    kind: memory
    op: load
    pe: pe0
    config:
      view:
        shape: [1, 16, 16]
        offsets: [0, 0, 0]

  - id: load1
    kind: memory
    op: load
    pe: pe0
    config:
      view:
        shape: [1, 16, 16]
        offsets: [1, 0, 0]

edges:
  - from: tensor0
    to: load0
    kind: data

  - from: tensor0
    to: load1
    kind: data
",
    )
    .unwrap();

    let timetable: Rc<dyn Dispatch> =
        Rc::new(Timetable::new(engine.top(), timetable_file, &platform).unwrap());
    platform.attach_dispatcher(&timetable);
    let err = engine.run().unwrap_err();
    assert!(
        format!("{err}")
            .contains("PE cannot do memory access of 1024 as it only has SRAM with 128 bytes.")
    );
}
