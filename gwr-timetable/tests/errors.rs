// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::task::MemoryOp;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::types::{EdgeKind, EdgeSection, Graph, MemoryConfigSection, NodeSection};

fn create_default_graph() -> (Rc<Platform>, Graph) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    (
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
        Graph::from_string(
            "
nodes:
  - id: node0
    kind: memory
    op: load
    pe: pe0
    config:
      addr: 0
      num_bytes: 0

  - id: node1
    kind: memory
    op: load
    pe: pe0
    config:
      addr: 0
      num_bytes: 0

edges:
  - id: edge0
    from: node0
    to: node0
    kind: data
",
        )
        .unwrap(),
    )
}

#[test]
fn graph() {
    let (platform, graph) = create_default_graph();
    graph.validate(&platform).unwrap();
}

// Node errors

#[test]
#[should_panic(expected = "Node node2 contains invalid PE ID pe1")]
fn invalid_node_pe() {
    let (platform, mut graph) = create_default_graph();
    graph.nodes.push(NodeSection::Memory {
        id: "node2".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe1".to_string()),
        config: MemoryConfigSection {
            addr: 0,
            num_bytes: 0,
        },
    });

    graph.validate(&platform).unwrap();
}

#[test]
#[should_panic(expected = "Duplicate node ID node1")]
fn duplicate_node_id() {
    let (platform, mut graph) = create_default_graph();
    graph.nodes.push(NodeSection::Memory {
        id: "node1".to_string(),
        op: MemoryOp::Store,
        pe: Some("pe0".to_string()),
        config: MemoryConfigSection {
            addr: 0,
            num_bytes: 0,
        },
    });

    graph.validate(&platform).unwrap();
}

// Edge errors

#[test]
#[should_panic(expected = "Edge contains invalid from Node ID node2")]
fn invalid_from_edge_pe() {
    let (platform, mut graph) = create_default_graph();
    graph.edges.push(EdgeSection {
        from: "node2".to_string(),
        to: "node0".to_string(),
        kind: EdgeKind::Data,
    });

    graph.validate(&platform).unwrap();
}

#[test]
#[should_panic(expected = "Edge contains invalid to Node ID node2")]
fn invalid_to_edge_pe() {
    let (platform, mut graph) = create_default_graph();
    graph.edges.push(EdgeSection {
        from: "node0".to_string(),
        to: "node2".to_string(),
        kind: EdgeKind::Data,
    });

    graph.validate(&platform).unwrap();
}

#[test]
#[should_panic(expected = "PE cannot do memory access of 256 as it only has SRAM with 128 bytes.")]
fn meory_op_too_bit() {
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
    let graph = Graph::from_string(
        "
nodes:
  - id: node0
    kind: memory
    op: load
    pe: pe0
    config:
      addr: 0
      num_bytes: 128

  - id: node1
    kind: memory
    op: load
    pe: pe0
    config:
      addr: 0
      num_bytes: 256

edges:
  - id: edge0
    from: node0
    to: node0
    kind: data
",
    )
    .unwrap();

    let timetable: Rc<dyn Dispatch> =
        Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    platform.attach_dispatcher(&timetable);
    engine.run().unwrap();
}
