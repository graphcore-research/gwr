// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
#![cfg(feature = "functional")]
use gwr_models::memory::FunctionalMemory;

#[test]
fn loads_and_checks_real_values_and_rejects_uninitialized_data() {
    let memory = FunctionalMemory::default();
    assert!(memory.read_f32(16, 1).is_err());
    memory.write_f32(16, &[1.25, -2.0, 3.5]).unwrap();
    assert_eq!(memory.read_f32(20, 2).unwrap(), [-2.0, 3.5]);
    let check = memory.check_f32(16, &[1.25, -2.0, 3.5], 0.0, 0.0).unwrap();
    assert_eq!(check.elements, 3);
    assert_eq!(check.max_absolute_error, 0.0);
    assert!(memory.check_f32(16, &[1.0], 0.0, 0.0).is_err());
    assert!(memory.check_f32(16, &[1.25], f32::NAN, 0.0).is_err());
}

#[test]
fn comparison_does_not_overflow_and_false_pass() {
    let memory = FunctionalMemory::default();
    memory.write_f32(0, &[f32::MAX]).unwrap();
    assert!(memory.check_f32(0, &[-f32::MAX], 0.0, 1.5).is_err());
    memory.write_f32(0, &[f32::NAN]).unwrap();
    assert!(memory.check_f32(0, &[f32::NAN], 0.0, 0.0).is_err());
    memory.write_f32(0, &[f32::INFINITY]).unwrap();
    assert!(memory.check_f32(0, &[f32::INFINITY], 0.0, 0.0).is_err());
}

#[test]
fn address_overflow_fails_without_partial_initialization() {
    let memory = FunctionalMemory::default();
    assert!(memory.write_f32(u64::MAX - 2, &[1.0]).is_err());
    assert!(memory.read_f32(u64::MAX - 2, 1).is_err());
    assert!(memory.read_f32(0, usize::MAX).is_err());
}

#[test]
fn unaligned_page_crossing_preserves_missing_bytes_and_neighbors() {
    let memory = FunctionalMemory::default();
    memory.write_f32(65533, &[1.25, -7.5, 9.]).unwrap();
    assert_eq!(memory.read_f32(65533, 3).unwrap(), [1.25, -7.5, 9.]);
    assert!(memory.read_f32(65532, 1).is_err());
    assert!(memory.read_f32(65542, 1).is_err());
    memory.write_f32(65537, &[6.]).unwrap();
    assert_eq!(memory.read_f32(65533, 3).unwrap(), [1.25, 6., 9.]);
    memory.write_f32(u64::MAX - 3, &[2.]).unwrap();
    assert_eq!(memory.read_f32(u64::MAX - 3, 1).unwrap(), [2.]);
}
