// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;
use std::vec;

use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::{MemoryOp, Task};
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

#[test]
fn timetable_file_validation_without_platform() {
    let (_, _, timetable_file) = create_default_timetable_file();
    timetable_file.validate().unwrap();
}

#[test]
fn control_edges_are_ignored_by_scheduler() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "load0".to_string(),
        to: "load1".to_string(),
        kind: EdgeKind::Control,
    });

    timetable_file.validate().unwrap();
    let timetable = Timetable::new(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![1, 2]);

    timetable.set_task_completed(1).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);
}

#[test]
fn control_edges_ignore_data_port_suffixes() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "load0.99".to_string(),
        to: "load1.99".to_string(),
        kind: EdgeKind::Control,
    });

    timetable_file.validate().unwrap();
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
            from: "load0".to_string(),
            to: "gate".to_string(),
            kind: EdgeKind::Control,
        },
        EdgeSection {
            from: "gate".to_string(),
            to: "load1".to_string(),
            kind: EdgeKind::Control,
        },
    ]);

    let timetable = Timetable::new(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![1, 2]);

    timetable.set_task_completed(1).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);
}

#[test]
fn semantic_validation_rejects_tensor_to_tensor_edges() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: tensor0
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [1] }
  - id: tensor1
    kind: tensor
    config: { addr: 4, dtype: fp32, shape: [1] }
edges:
  - { from: tensor0, to: tensor1, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(
        format!("{err}")
            .contains("Invalid edge from Tensor node 'tensor0' to Tensor node 'tensor1'")
    );
}

#[test]
fn semantic_validation_rejects_compute_to_compute_data_edges() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer
    kind: compute
    op: add
    input_views: []
    output_views: [null]
  - id: consumer
    kind: compute
    op: add
    input_views: [null]
    output_views: []
edges:
  - from: producer
    to: consumer
    kind: data
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("output 0 is not connected to a Tensor node"));
}

#[test]
fn semantic_validation_rejects_overlapping_compute_reads_and_writes() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0x1000, dtype: fp32, shape: [4] }
  - id: compute
    kind: compute
    op: add
    input_views: [null]
    output_views: [null]
  - id: output
    kind: tensor
    config: { addr: 0x1008, dtype: fp32, shape: [4] }
edges:
  - { from: input, to: compute, kind: data }
  - { from: compute, to: output, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    let message = format!("{err}");
    assert!(message.contains("Node 'compute' reads tensor 'input'"));
    assert!(message.contains("writes tensor 'output' to overlapping range"));
}

#[test]
fn semantic_validation_accepts_compute_views_in_adjacent_bytes() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0x1000, dtype: int4, shape: [4] }
  - id: compute
    kind: compute
    op: add
    input_views:
      - { offsets: [0], shape: [1] }
    output_views:
      - { offsets: [2], shape: [1] }
  - id: output
    kind: tensor
    config: { addr: 0x1000, dtype: int4, shape: [4] }
edges:
  - { from: input, to: compute, kind: data }
  - { from: compute, to: output, kind: data }
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_rejects_overlapping_sub_byte_compute_views() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0x1000, dtype: int4, shape: [2] }
  - id: compute
    kind: compute
    op: add
    input_views:
      - { offsets: [1], shape: [1] }
    output_views:
      - { offsets: [0], shape: [1] }
  - id: output
    kind: tensor
    config: { addr: 0x1000, dtype: int4, shape: [2] }
edges:
  - { from: input, to: compute, kind: data }
  - { from: compute, to: output, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("writes tensor 'output' to overlapping range"));
}

#[test]
fn semantic_validation_rejects_overlapping_memory_reads_and_writes() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0x1000, dtype: fp32, shape: [4] }
  - id: store
    kind: memory
    op: store
    config: { view: null }
  - id: output
    kind: tensor
    config: { addr: 0x1008, dtype: fp32, shape: [4] }
edges:
  - { from: input, to: store, kind: data }
  - { from: store, to: output, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    let message = format!("{err}");
    assert!(message.contains("Node 'store' reads tensor 'input'"));
    assert!(message.contains("writes tensor 'output' to overlapping range"));
}

#[test]
fn semantic_validation_rejects_overlapping_compute_writes() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: compute
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null, null]
  - id: output0
    kind: tensor
    config: { addr: 0x1000, dtype: fp32, shape: [2] }
  - id: output1
    kind: tensor
    config: { addr: 0x1004, dtype: fp32, shape: [2] }
edges:
  - { from: compute.0, to: output0, kind: data }
  - { from: compute.1, to: output1, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    let message = format!("{err}");
    assert!(message.contains("Node 'compute' writes tensor 'output0'"));
    assert!(message.contains("tensor 'output1' to overlapping range"));
}

#[test]
fn semantic_validation_accepts_adjacent_compute_writes() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: compute
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null, null]
  - id: output0
    kind: tensor
    config: { addr: 0x1000, dtype: fp32, shape: [1] }
  - id: output1
    kind: tensor
    config: { addr: 0x1004, dtype: fp32, shape: [1] }
edges:
  - { from: compute.0, to: output0, kind: data }
  - { from: compute.1, to: output1, kind: data }
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_rejects_zero_sized_compute_views() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [4] }
  - id: compute
    kind: compute
    op: add
    input_views:
      - { offsets: [0], shape: [0] }
    output_views: []
edges:
  - { from: input, to: compute, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("input view on node 'compute' has zero size in dim 0"));
}

#[test]
fn semantic_validation_rejects_zero_sized_memory_views() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [4] }
  - id: load
    kind: memory
    op: load
    config:
      view:
        offsets: [0]
        shape: [0]
edges:
  - { from: input, to: load, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("Load view on node 'load' has zero size in dim 0"));
}

#[test]
fn semantic_validation_rejects_zero_sized_tensors() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: empty
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [2, 0] }
edges: []
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("Tensor 'empty' has zero size in dim 1"));
}

#[test]
fn semantic_validation_rejects_tensor_address_overflow() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: overflowing
    kind: tensor
    config: { addr: 18446744073709551615, dtype: fp32, shape: [1] }
edges: []
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(
        format!("{err}")
            .contains("Tensor 'overflowing' range overflows the physical address space")
    );
}

#[test]
fn semantic_validation_accepts_tensor_ending_at_final_physical_byte() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: top_exclusive
    kind: tensor
    config: { addr: 18446744073709551614, dtype: int8, shape: [1] }
edges: []
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_rejects_tensor_at_final_physical_byte() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: final_byte
    kind: tensor
    config: { addr: 18446744073709551615, dtype: int8, shape: [1] }
edges: []
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(
        format!("{err}").contains("Tensor 'final_byte' range overflows the physical address space")
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
fn memory_node_rejects_unknown_view_field() {
    let err = TimetableFile::from_string(
        "
nodes:
  - id: load0
    kind: memory
    op: load
    pe: pe0
    config:
      view:
        shape: [4]
        offsets: [0]
        stride: [1]

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
fn load_with_data_output_is_rejected() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [1] }
  - id: load
    kind: memory
    op: load
    config: {}
  - id: output
    kind: tensor
    config: { addr: 4, dtype: fp32, shape: [1] }
edges:
  - { from: input, to: load, kind: data }
  - { from: load, to: output, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("1 data edges connect from Load node 'load'"));
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
        from: "tensor0".to_string(),
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
fn invalid_edge_endpoint_syntax() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "tensor0.invalid".to_string(),
        to: "load0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("Unable to parse edge id 'tensor0.invalid'"));
}

#[test]
fn duplicate_explicit_edge_port_after_implicit_port() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "tensor0".to_string(),
        to: "load0.0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("input edge index 0 is connected more than once"));
}

#[test]
fn sparse_explicit_tensor_port_and_implicit_port_are_allowed() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].from = "tensor0.5".to_string();

    timetable_file.validate().unwrap();
    let timetable = Timetable::new(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![1, 2]);
}

#[test]
fn maximum_tensor_port_does_not_materialize_intervening_ports() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].from = format!("tensor0.{}", usize::MAX);

    timetable_file.validate().unwrap();
    Timetable::new(&top, timetable_file, &platform).unwrap();
}

#[test]
fn duplicate_sparse_tensor_ports_are_rejected() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].from = "tensor0.5".to_string();
    timetable_file.edges[1].from = "tensor0.5".to_string();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("output edge index 5 is connected more than once"));
}

#[test]
fn explicit_edge_port_index_is_checked_before_tracking_occupancy() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0, dtype: fp32, shape: [1] }
  - id: compute
    kind: compute
    op: add
    input_views:
      -
    output_views: []
edges:
  - { from: input, to: compute.1000000000, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(
        format!("{err}").contains("Node 'compute' input edge index 1000000000 is out of range")
    );
}

#[test]
fn sub_byte_memory_views_use_physical_byte_range() {
    let (top, platform, _) = create_default_timetable_file();
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: tensor0
    kind: tensor
    config: { addr: 0x1000, dtype: int4, shape: [4] }
  - id: load0
    kind: memory
    op: load
    pe: pe0
    config:
      view:
        offsets: [1]
        shape: [2]
edges:
  - { from: tensor0, to: load0, kind: data }
",
    )
    .unwrap();

    let timetable = Timetable::new(&top, timetable_file, &platform).unwrap();
    let Task::MemoryTask { config } = timetable.task_by_id(1).unwrap() else {
        panic!("load0 should produce a memory task");
    };
    assert_eq!(config.addr, 0x1000);
    assert_eq!(config.num_bytes, 2);
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
