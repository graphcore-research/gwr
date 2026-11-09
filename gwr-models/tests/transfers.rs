// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_models::transfer::TransferTaskConfig;
#[test]
fn bounded_chunks_pair_differently_strided_views_in_order() {
    let source = TensorView::new(
        Tensor::new(&[3, 4], &DataType::Int8, 0).unwrap(),
        &[3, 2],
        &[0, 1],
    )
    .unwrap();
    let destination = TensorView::new(
        Tensor::new(&[3, 5], &DataType::Int8, 100).unwrap(),
        &[3, 2],
        &[0, 2],
    )
    .unwrap();
    let task = TransferTaskConfig {
        id: "copy".into(),
        source,
        destination,
        buffer_bytes: 1,
    };
    task.validate().unwrap();
    assert_eq!(
        task.chunks().collect::<Vec<_>>(),
        vec![
            (1..2, 102..103),
            (2..3, 103..104),
            (5..6, 107..108),
            (6..7, 108..109),
            (9..10, 112..113),
            (10..11, 113..114)
        ]
    );
    assert_eq!(task.planned_traffic_bytes().unwrap(), (6, 6));
}
#[test]
fn full_size_copy_plan_is_lazy_and_bounded() {
    let task = TransferTaskConfig {
        id: "copy".into(),
        source: TensorView::new_full(Tensor::new(&[1_000_000_000], &DataType::Int8, 0).unwrap()),
        destination: TensorView::new_full(
            Tensor::new(&[1_000_000_000], &DataType::Int8, 2_000_000_000).unwrap(),
        ),
        buffer_bytes: 4096,
    };
    task.validate().unwrap();
    assert_eq!(
        task.chunks().take(2).collect::<Vec<_>>(),
        vec![
            (0..4096, 2_000_000_000..2_000_004_096),
            (4096..8192, 2_000_004_096..2_000_008_192)
        ]
    );
}
