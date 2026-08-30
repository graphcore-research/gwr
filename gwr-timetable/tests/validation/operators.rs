// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::*;

#[test]
fn rejects_invalid_operator_inputs() {
    let timetable_file = timetable(
        vec![
            tensor("input", 0, DataType::Fp32, &[4, 4]),
            NodeSection::Compute {
                id: "gemm".to_string(),
                op: ComputeOp::Gemm,
                pe: None,
                input_views: vec![None],
                output_views: vec![None],
            },
            tensor("output", 64, DataType::Fp32, &[4, 4]),
        ],
        vec![data_edge("input", "gemm"), data_edge("gemm", "output")],
    );

    let error = timetable_file.validate().unwrap_err();

    assert!(error.to_string().contains("Compute node 'gemm': Gemm"));
    assert!(error.to_string().contains("input tensors found - expected"));
}

#[test]
fn rejects_overflowing_maxpool_parameters() {
    let timetable_file = timetable(
        vec![
            tensor("input", 0, DataType::Int8, &[1, 1, 2]),
            NodeSection::Compute {
                id: "maxpool".to_string(),
                op: ComputeOp::MaxPool(OperatorMaxPool {
                    dilations: Some(vec![usize::MAX]),
                    ..OperatorMaxPool::new(&[2])
                }),
                pe: None,
                input_views: vec![None],
                output_views: vec![None],
            },
            tensor("output", 2, DataType::Int8, &[1, 1, 1]),
        ],
        vec![
            data_edge("input", "maxpool"),
            data_edge("maxpool", "output"),
        ],
    );

    let error = timetable_file.validate().unwrap_err().to_string();
    assert!(error.contains("Compute node 'maxpool': MaxPool"));
    assert!(error.contains("effective kernel size overflows"));
}
