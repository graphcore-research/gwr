// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::*;

#[test]
fn rejects_zero_sized_views() {
    let timetable_file = timetable(
        vec![
            tensor("input", 0, DataType::Fp32, &[4]),
            compute("compute", vec![view(&[0], &[0])], vec![]),
        ],
        vec![data_edge("input", "compute")],
    );

    let err = timetable_file.validate().unwrap_err();
    assert!(
        format!("{err}")
            .contains("input view on node 'compute': Shape [0] has zero size in dimension 0")
    );
}

#[test]
fn rejects_zero_sized_tensors() {
    let timetable_file = timetable(vec![tensor("empty", 0, DataType::Fp32, &[2, 0])], vec![]);

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains("Tensor 'empty': Shape [2, 0] has zero size in dimension 1"));
}

#[test]
fn rejects_tensor_address_overflow() {
    let timetable_file = timetable(
        vec![tensor("overflowing", u64::MAX, DataType::Fp32, &[1])],
        vec![],
    );

    let err = timetable_file.validate().unwrap_err();
    assert!(format!("{err}").contains(
        "Tensor at 0xffffffffffffffff with size 4 bytes exceeds the physical address space"
    ));
}

#[test]
fn accepts_a_tensor_ending_at_the_final_physical_byte() {
    let timetable_file = timetable(
        vec![tensor("top_exclusive", u64::MAX - 1, DataType::Int8, &[2])],
        vec![],
    );

    timetable_file.validate().unwrap();
}

#[test]
fn accepts_a_tensor_at_the_final_physical_byte() {
    let timetable_file = timetable(
        vec![tensor("final_byte", u64::MAX, DataType::Int8, &[1])],
        vec![],
    );

    timetable_file.validate().unwrap();
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
fn compute_input_view_outside_tensor() {
    let (top, platform, mut timetable_file) = create_default_timetable_file();
    let NodeSection::Compute { input_views, .. } = &mut timetable_file.nodes[2] else {
        panic!("expected compute node");
    };
    input_views[0] = Some(TensorViewSection {
        shape: vec![3, 10, 10],
        offsets: vec![1, 1, 1],
    });

    let err = build_timetable(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains(
        "input view on node 'add0': Tensor view range 1..4 is out of range for dimension 0"
    ));
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

    let err = build_timetable(&top, timetable_file, &platform).unwrap_err();
    assert!(format!("{err}").contains(
        "output view on node 'add0': Tensor view range 0..100 is out of range for dimension 2"
    ));
}

#[test]
fn rejects_a_view_on_a_disconnected_tensor_index() {
    let timetable_file = timetable(
        vec![compute("compute", vec![view(&[0], &[1]), None], vec![])],
        vec![],
    );

    let error = timetable_file.validate().unwrap_err().to_string();
    assert_eq!(
        error,
        "Compute node 'compute' declares an input view for disconnected tensor index 0"
    );
}

#[test]
fn accepts_an_empty_optional_tensor() {
    timetable(vec![compute("compute", vec![None], vec![None])], vec![])
        .validate()
        .unwrap();
}
