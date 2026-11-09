// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_engine::types::DeviceId;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_models::roofline::accounting::Operation;
use gwr_models::roofline::traffic::ViewTraffic;

#[test]
fn aligned_lsu_traffic_charges_full_packets_and_headers() {
    let mut map = MemoryMap::new();
    map.insert(0, 4096, DeviceId(1)).unwrap();
    for (address, bytes, expected) in [(0, 1, 32), (4, 32, 64), (32, 32, 32), (31, 2, 64)] {
        let view = TensorView::new_full(Tensor::new(&[bytes], &DataType::Int8, address).unwrap());
        for operation in [Operation::Read, Operation::WriteNonPosted] {
            let plan = ViewTraffic::new_lsu_aligned(&view, operation, &map, 32, 8).unwrap();
            let traffic = plan.destinations()[0].traffic();
            assert_eq!(traffic.payload, expected);
            assert_eq!(traffic.transactions, expected / 32);
            assert_eq!(
                traffic.request_wire + traffic.response_wire,
                expected + (expected / 32) * 16
            );
        }
    }
}

#[test]
fn aligned_lsu_traffic_preserves_duplicate_strided_issues() {
    let mut map = MemoryMap::new();
    map.insert(0, 4096, DeviceId(1)).unwrap();
    let tensor = Tensor::new(&[4, 2], &DataType::Int8, 0).unwrap();
    let view = TensorView::new(tensor, &[4, 1], &[0, 0]).unwrap();
    let plan = ViewTraffic::new_lsu_aligned(&view, Operation::Read, &map, 32, 8).unwrap();
    assert_eq!(plan.destinations()[0].traffic().payload, 128);
}

#[test]
fn aligned_lsu_traffic_rejects_packets_crossing_unmapped_boundaries() {
    let mut map = MemoryMap::new();
    map.insert(1, 32, DeviceId(1)).unwrap();
    let view = TensorView::new_full(Tensor::new(&[1], &DataType::Int8, 1).unwrap());
    assert!(ViewTraffic::new_lsu_aligned(&view, Operation::Read, &map, 32, 8).is_err());
}

#[test]
fn packed_dense_trillion_rows_are_counted_without_iterator_coalescing() {
    let count = 1_000_000_000_000usize;
    let tensor = Tensor::new(&[count, 2], &DataType::Fp4, 0).unwrap();
    let view = TensorView::new(tensor, &[count, 1], &[0, 0]).unwrap();
    let mut map = MemoryMap::new();
    map.insert(0, count as u64, DeviceId(1)).unwrap();
    let traffic = ViewTraffic::new_lsu_aligned(&view, Operation::Read, &map, 32, 8).unwrap();
    assert_eq!(traffic.destinations()[0].traffic().payload, count as u128);
}
