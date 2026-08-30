// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::*;

#[test]
fn builds_a_valid_timetable() {
    let (top, platform, timetable_file) = create_default_timetable_file();
    build_timetable(&top, timetable_file, &platform).unwrap();
}

#[test]
fn validates_without_a_platform() {
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
    let timetable = build_timetable(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);

    timetable.set_task_completed(2).unwrap();
    assert!(timetable.ready_task_indices("pe0").unwrap().1.is_empty());
}

#[test]
fn control_edges_ignore_data_tensor_indices() {
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

    let timetable = build_timetable(&top, timetable_file, &platform).unwrap();
    assert_eq!(timetable.ready_task_indices("pe0").unwrap().1, vec![2]);

    timetable.set_task_completed(2).unwrap();
    assert!(timetable.ready_task_indices("pe0").unwrap().1.is_empty());
}

#[test]
fn rejects_data_dependency_cycles() {
    let timetable_file = timetable(
        vec![
            tensor("tensor0", 0, DataType::Int8, &[1]),
            compute("compute0", vec![None], vec![None]),
            tensor("tensor1", 1, DataType::Int8, &[1]),
            compute("compute1", vec![None], vec![None]),
        ],
        vec![
            data_edge("tensor0", "compute0"),
            data_edge("compute0", "tensor1"),
            data_edge("tensor1", "compute1"),
            data_edge("compute1", "tensor0"),
        ],
    );

    let err = timetable_file.validate().unwrap_err();
    assert_eq!(
        format!("{err}"),
        "Data dependency graph contains a cycle; unresolved nodes: \
'tensor0', 'compute0', 'tensor1', 'compute1'"
    );
}

#[test]
fn ignores_control_dependency_cycles() {
    let timetable_file = timetable(
        vec![
            compute("compute0", vec![], vec![]),
            compute("compute1", vec![], vec![]),
        ],
        vec![
            control_edge("compute0", "compute1"),
            control_edge("compute1", "compute0"),
        ],
    );

    timetable_file.validate().unwrap();
}
