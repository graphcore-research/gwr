// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::types::DeviceId;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_models::roofline::accounting::Operation;
use gwr_models::roofline::traffic::ViewTraffic;

fn check(view: &TensorView, map: &MemoryMap, maximum: u128) {
    for operation in [
        Operation::Read,
        Operation::WriteNonPosted,
        Operation::WritePosted,
    ] {
        let exact = ViewTraffic::new_exact(view, operation, map, maximum, 8);
        let analytical = ViewTraffic::new_analytical(view, operation, map, maximum, 8);
        assert_eq!(exact.is_ok(), analytical.is_ok(), "{view:?}");
        if let Ok(exact) = exact {
            let analytical = analytical.unwrap();
            assert_eq!(exact.destinations().len(), analytical.destinations().len());
            for (a, b) in exact.destinations().iter().zip(analytical.destinations()) {
                assert_eq!(a.device(), b.device());
                assert_eq!(a.traffic(), b.traffic(), "{view:?}");
                assert_eq!(a.runs(), b.runs(), "{view:?}");
            }
        }
    }
}

#[test]
fn analytical_counts_match_exact_across_shapes_offsets_and_memory_boundaries() {
    for dtype in [DataType::Int8, DataType::Int16, DataType::Fp32] {
        let tensor = Tensor::new(&[3, 4, 5], &dtype, 11).unwrap();
        let total = tensor.num_bytes() as u64;
        for first in 0..3 {
            for rows in 1..=4 {
                for width in 1..=5 {
                    let view = TensorView::new(
                        tensor.clone(),
                        &[3 - first, rows, width],
                        &[first, 4 - rows, 5 - width],
                    )
                    .unwrap();
                    for cut in [1, 7, total / 2, total - 1] {
                        let mut map = MemoryMap::new();
                        map.insert(11, cut, DeviceId(9)).unwrap();
                        map.insert(11 + cut, total - cut, DeviceId(1)).unwrap();
                        for maximum in [1, 3, 16, 64] {
                            check(&view, &map, maximum);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn trillion_fragment_view_is_counted_without_fragment_expansion() {
    let count = 1_000_000_000_000usize;
    let tensor = Tensor::new(&[count, 2], &DataType::Int8, 0).unwrap();
    let view = TensorView::new(tensor, &[count, 1], &[0, 0]).unwrap();
    let mut map = MemoryMap::new();
    // Split through the middle of the repeated layout, not at its end.
    map.insert(0, count as u64, DeviceId(1)).unwrap();
    map.insert(count as u64, count as u64, DeviceId(2)).unwrap();
    let plan = ViewTraffic::new_analytical(&view, Operation::Read, &map, 64, 8).unwrap();
    for child in plan.destinations() {
        assert_eq!(child.runs().len(), 1);
        assert_eq!(child.traffic().payload, 500_000_000_000);
        assert_eq!(child.traffic().transactions, 500_000_000_000);
        assert_eq!(child.traffic().request_wire, 4_000_000_000_000);
    }
}

#[test]
fn touched_unmapped_bytes_fail_and_untouched_gaps_are_ignored() {
    let tensor = Tensor::new(&[3, 4], &DataType::Int8, 0).unwrap();
    let view = TensorView::new(tensor.clone(), &[3, 2], &[0, 0]).unwrap();
    let mut map = MemoryMap::new();
    for start in [0, 4, 8] {
        map.insert(start, 2, DeviceId(1)).unwrap();
    }
    check(&view, &map, 64);
    assert!(
        ViewTraffic::new_analytical(&TensorView::new_full(tensor), Operation::Read, &map, 64, 8)
            .is_err()
    );
    assert!(map.unmap(4));
    assert!(ViewTraffic::new_analytical(&view, Operation::Read, &map, 64, 8).is_err());
}

#[test]
fn packed_contiguous_and_bounded_fragmented_coverage_are_supported() {
    let mut map = MemoryMap::new();
    map.insert(0, 100, DeviceId(1)).unwrap();
    let contiguous = TensorView::new(
        Tensor::new(&[2, 3], &DataType::Int4, 0).unwrap(),
        &[2, 1],
        &[0, 0],
    )
    .unwrap();
    check(&contiguous, &map, 64);
    let fragmented = TensorView::new(
        Tensor::new(&[2, 5], &DataType::Int4, 0).unwrap(),
        &[2, 1],
        &[0, 0],
    )
    .unwrap();
    check(&fragmented, &map, 64);
    assert!(ViewTraffic::new_exact(&fragmented, Operation::Read, &map, 64, 8).is_ok());
}

#[test]
fn last_address_and_invalid_shaping_are_checked() {
    let view = TensorView::new_full(Tensor::new(&[1], &DataType::Int8, u64::MAX).unwrap());
    let mut map = MemoryMap::new();
    map.insert(u64::MAX, 1, DeviceId(1)).unwrap();
    check(&view, &map, 64);
    assert!(ViewTraffic::new_analytical(&view, Operation::Read, &map, 0, 8).is_err());
    assert!(ViewTraffic::new_analytical(&view, Operation::Read, &map, 1, u128::MAX).is_err());
}
