// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt::Write as _;
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
fn timetable_file_validation_without_platform() {
    let (_, _, timetable_file) = create_default_timetable_file();
    timetable_file.validate().unwrap();
}

#[test]
fn control_edges_are_ignored_by_scheduler() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "tensor2".to_string(),
        to: "add0".to_string(),
        kind: EdgeKind::Control,
    });

    timetable_file.validate().unwrap();
    let timetable = Timetable::new(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);

    timetable.set_task_completed(2).unwrap();
    assert!(timetable.ready_task_indices("pe0").unwrap().1.is_empty());
}

#[test]
fn control_edges_ignore_data_port_suffixes() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges.push(EdgeSection {
        from: "tensor2.99".to_string(),
        to: "add0.99".to_string(),
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
fn semantic_validation_rejects_data_dependency_cycles() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: tensor0
    kind: tensor
    config: { addr: 0, dtype: int8, shape: [1] }
  - id: compute0
    kind: compute
    op:
      custom:
        name: compute0
        machine_ops: {}
    input_views: [null]
    output_views: [null]
  - id: tensor1
    kind: tensor
    config: { addr: 1, dtype: int8, shape: [1] }
  - id: compute1
    kind: compute
    op:
      custom:
        name: compute1
        machine_ops: {}
    input_views: [null]
    output_views: [null]
edges:
  - { from: tensor0, to: compute0, kind: data }
  - { from: compute0, to: tensor1, kind: data }
  - { from: tensor1, to: compute1, kind: data }
  - { from: compute1, to: tensor0, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert_eq!(
        format!("{err}"),
        "Data dependency graph contains a cycle; unresolved nodes: \
'tensor0', 'compute0', 'tensor1', 'compute1'"
    );
}

#[test]
fn semantic_validation_ignores_control_dependency_cycles() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: compute0
    kind: compute
    op:
      custom:
        name: compute0
        machine_ops: {}
    input_views: []
    output_views: []
  - id: compute1
    kind: compute
    op:
      custom:
        name: compute1
        machine_ops: {}
    input_views: []
    output_views: []
edges:
  - { from: compute0, to: compute1, kind: control }
  - { from: compute1, to: compute0, kind: control }
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
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
fn semantic_validation_accepts_disjoint_strided_compute_views() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [2, 4] }
  - id: compute
    kind: compute
    op: add
    input_views:
      - { offsets: [0, 0], shape: [2, 2] }
    output_views:
      - { offsets: [0, 2], shape: [2, 2] }
  - id: output
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [2, 4] }
edges:
  - { from: input, to: compute, kind: data }
  - { from: compute, to: output, kind: data }
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_rejects_overlapping_strided_compute_views() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [2, 4] }
  - id: compute
    kind: compute
    op: add
    input_views:
      - { offsets: [0, 0], shape: [2, 2] }
    output_views:
      - { offsets: [0, 1], shape: [2, 2] }
  - id: output
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [2, 4] }
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
fn semantic_validation_short_circuits_large_strided_overlap() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [100000000, 2] }
  - id: compute
    kind: compute
    op: add
    input_views:
      - { offsets: [0, 0], shape: [100000000, 1] }
    output_views:
      - { offsets: [0, 0], shape: [100000000, 1] }
  - id: output
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [100000000, 2] }
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
fn semantic_validation_rejects_overlapping_unordered_producers() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer0
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null]
  - id: producer1
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null]
  - id: result
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
edges:
  - { from: producer0, to: result.0, kind: data }
  - { from: producer1, to: result.1, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    let message = format!("{err}");
    assert!(message.contains("Nodes 'producer0' and 'producer1'"));
    assert!(message.contains("write tensor 'result' to overlapping memory ranges"));
}

#[test]
fn semantic_validation_accepts_disjoint_unordered_producers() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer0
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [0], shape: [2] }
  - id: producer1
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [2], shape: [2] }
  - id: result
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
edges:
  - { from: producer0, to: result.0, kind: data }
  - { from: producer1, to: result.1, kind: data }
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_rejects_packed_byte_sharing_between_producers() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer0
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [0], shape: [1] }
  - id: producer1
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [1], shape: [1] }
  - id: result
    kind: tensor
    config: { addr: 0x1000, dtype: int4, shape: [2] }
edges:
  - { from: producer0, to: result.0, kind: data }
  - { from: producer1, to: result.1, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("write tensor 'result' to overlapping memory ranges"));
}

#[test]
fn semantic_validation_accepts_overlapping_ordered_producers() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer0
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null, null]
  - id: gate
    kind: tensor
    config: { addr: 0x2000, dtype: int8, shape: [4] }
  - id: producer1
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: [null]
    output_views: [null]
  - id: result
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
edges:
  - { from: producer0.0, to: gate, kind: data }
  - { from: producer0.1, to: result.0, kind: data }
  - { from: gate, to: producer1, kind: data }
  - { from: producer1, to: result.1, kind: data }
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_rejects_unordered_writes_to_aliased_tensors() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer0
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null]
  - id: output0
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
  - id: producer1
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null]
  - id: output1
    kind: tensor
    config: { addr: 0x1002, dtype: int8, shape: [4] }
edges:
  - { from: producer0, to: output0, kind: data }
  - { from: producer1, to: output1, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    let message = format!("{err}");
    assert!(message.contains("Nodes 'producer0' and 'producer1'"));
    assert!(message.contains("write tensors 'output0' and 'output1'"));
    assert!(message.contains("0x1000..0x1004"));
    assert!(message.contains("0x1002..0x1006"));
}

#[test]
fn semantic_validation_accepts_disjoint_writes_to_aliased_tensors() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer0
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [0], shape: [2] }
  - id: output0
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
  - id: producer1
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [2], shape: [2] }
  - id: output1
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
edges:
  - { from: producer0, to: output0, kind: data }
  - { from: producer1, to: output1, kind: data }
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_rejects_packed_byte_sharing_between_aliased_tensors() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer0
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [0], shape: [1] }
  - id: output0
    kind: tensor
    config: { addr: 0x1000, dtype: int4, shape: [2] }
  - id: producer1
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [1], shape: [1] }
  - id: output1
    kind: tensor
    config: { addr: 0x1000, dtype: int4, shape: [2] }
edges:
  - { from: producer0, to: output0, kind: data }
  - { from: producer1, to: output1, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("write tensors 'output0' and 'output1'"));
}

#[test]
fn semantic_validation_rejects_strided_writes_to_aliased_tensors() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer0
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [0, 0], shape: [2, 2] }
  - id: output0
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [2, 4] }
  - id: producer1
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views:
      - { offsets: [0, 1], shape: [2, 2] }
  - id: output1
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [2, 4] }
edges:
  - { from: producer0, to: output0, kind: data }
  - { from: producer1, to: output1, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("write tensors 'output0' and 'output1'"));
}

#[test]
fn semantic_validation_accepts_dependency_ordered_aliased_writes() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: producer0
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null, null]
  - id: gate
    kind: tensor
    config: { addr: 0x2000, dtype: int8, shape: [1] }
  - id: output0
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
  - id: producer1
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: [null]
    output_views: [null]
  - id: output1
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
edges:
  - { from: producer0.0, to: gate, kind: data }
  - { from: producer0.1, to: output0, kind: data }
  - { from: gate, to: producer1, kind: data }
  - { from: producer1, to: output1, kind: data }
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_rejects_unordered_reads_and_writes_to_aliased_tensors() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
  - id: reader
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: [null]
    output_views: []
  - id: writer
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null]
  - id: output
    kind: tensor
    config: { addr: 0x1002, dtype: int8, shape: [4] }
edges:
  - { from: input, to: reader, kind: data }
  - { from: writer, to: output, kind: data }
",
    )
    .unwrap();

    let err = timetable_file.validate().unwrap_err();
    let message = format!("{err}");
    assert!(message.contains("Node 'reader' reads tensor 'input'"));
    assert!(message.contains("unordered node 'writer' writes tensor 'output'"));
    assert!(message.contains("0x1000..0x1004"));
    assert!(message.contains("0x1002..0x1006"));
}

#[test]
fn semantic_validation_accepts_dependency_ordered_aliased_read_and_write() {
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: writer
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: []
    output_views: [null, null]
  - id: gate
    kind: tensor
    config: { addr: 0x2000, dtype: int8, shape: [1] }
  - id: output
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
  - id: input
    kind: tensor
    config: { addr: 0x1000, dtype: int8, shape: [4] }
  - id: reader
    kind: compute
    op: { custom: { machine_ops: {} } }
    input_views: [null, null]
    output_views: []
edges:
  - { from: writer.0, to: gate, kind: data }
  - { from: writer.1, to: output, kind: data }
  - { from: gate, to: reader.0, kind: data }
  - { from: input, to: reader.1, kind: data }
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_indexes_many_disjoint_partitions_by_range() {
    const NUM_PARTITIONS: usize = 10_000;

    let mut yaml = String::from("nodes:\n");
    for index in 0..NUM_PARTITIONS {
        writeln!(
            yaml,
            "  - id: producer{index}\n    kind: compute\n    op: {{ custom: {{ machine_ops: {{}} }} }}\n    input_views: []\n    output_views:\n      - {{ offsets: [{index}], shape: [1] }}"
        )
        .unwrap();
    }
    writeln!(
        yaml,
        "  - id: output\n    kind: tensor\n    config: {{ addr: 0, dtype: int8, shape: [{NUM_PARTITIONS}] }}"
    )
    .unwrap();
    yaml.push_str("edges:\n");
    for index in 0..NUM_PARTITIONS {
        writeln!(
            yaml,
            "  - {{ from: producer{index}, to: output, kind: data }}"
        )
        .unwrap();
    }
    let timetable_file = TimetableFile::from_string(&yaml).unwrap();

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
    config: { addr: 18446744073709551614, dtype: int8, shape: [2] }
edges: []
",
    )
    .unwrap();

    timetable_file.validate().unwrap();
}

#[test]
fn semantic_validation_accepts_tensor_at_final_physical_byte() {
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

    timetable_file.validate().unwrap();
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
fn duplicate_explicit_edge_port_after_implicit_port() {
    let (_, _, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].to = "add0".to_string();
    timetable_file.edges.push(EdgeSection {
        from: "tensor0".to_string(),
        to: "add0.0".to_string(),
        kind: EdgeKind::Data,
    });

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("input edge index 0 is connected more than once"));
}

#[test]
fn sparse_explicit_tensor_port_and_implicit_port_are_allowed() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    timetable_file.edges[0].from = "tensor0.5".to_string();
    timetable_file.edges[1].from = "tensor0".to_string();

    timetable_file.validate().unwrap();
    let timetable = Timetable::new(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);
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
