// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::task::MemoryOp;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{
    DataType, EdgeKind, EdgeSection, MemoryConfigSection, NodeSection, TensorConfigSection,
    TimetableFile,
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
processing_elements:
  - name: pe0
    memory_map:
      ranges:
        - base_address: 0
          size_bytes: 0x1000_0000
          device: hbm0
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 0
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
      offset: 0
      num_elements: 150

  - id: load1
    kind: memory
    op: load
    pe: pe0
    config:
      offset: 150
      num_elements: 150

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
#[should_panic(expected = "Node 'node2' contains invalid PE ID 'pe1'")]
fn invalid_node_pe() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "node2".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe1".to_string()),
        config: MemoryConfigSection {
            offset: 0,
            num_elements: 0,
        },
    });

    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
#[should_panic(expected = "Duplicate Node ID 'load1'")]
fn duplicate_node_id() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "load1".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection {
            offset: 0,
            num_elements: 0,
        },
    });

    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
#[should_panic(expected = "0 edges connect into Load node")]
fn load_not_connected_to_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "node2".to_string(),
        op: MemoryOp::Load,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection {
            offset: 0,
            num_elements: 0,
        },
    });

    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
#[should_panic(expected = "0 edges connect from Store node")]
fn store_not_connected_to_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "node2".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection {
            offset: 0,
            num_elements: 0,
        },
    });

    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
#[should_panic(expected = "accesses memory outside Tensor node")]
fn load_outside_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "node2".to_string(),
        op: MemoryOp::Load,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection {
            offset: 10000,
            num_elements: 1,
        },
    });
    timetable_file.edges.push(EdgeSection {
        from: "tensor0".to_string(),
        to: "node2".to_string(),
        kind: EdgeKind::Data,
    });

    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
#[should_panic(expected = "accesses memory outside Tensor node")]
fn store_outside_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.nodes.push(NodeSection::Memory {
        id: "store0".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection {
            offset: 0,
            num_elements: 100,
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

    Timetable::new(&top, timetable_file, &platform).unwrap();
}

// Edge errors

#[test]
#[should_panic(expected = "Edge contains invalid from Node ID 'node2'")]
fn invalid_from_edge_pe() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "node2".to_string(),
        to: "load0".to_string(),
        kind: EdgeKind::Data,
    });

    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
#[should_panic(expected = "Edge contains invalid to Node ID 'node2'")]
fn invalid_to_edge_pe() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "load0".to_string(),
        to: "node2".to_string(),
        kind: EdgeKind::Data,
    });

    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
#[should_panic(expected = "PE cannot do memory access of 1024 as it only has SRAM with 128 bytes.")]
fn memory_op_too_big() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Rc::new(
        Platform::from_string(
            &engine,
            &clock,
            "
processing_elements:
  - name: pe0
    memory_map:
      ranges:
        - base_address: 0
          size_bytes: 0x1000_0000
          device: hbm0
    config:
      sram_bytes: 128

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 0

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
      offset: 0
      num_elements: 256

  - id: load1
    kind: memory
    op: load
    pe: pe0
    config:
      offset: 256
      num_elements: 256

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
    engine.run().unwrap();
}
