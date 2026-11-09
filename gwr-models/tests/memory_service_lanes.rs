// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Pure checked-accounting RED draft. No request acceptance follows from zero.
use gwr_models::memory::payload_by_lane;

#[test]
fn payload_lanes_conserve_unaligned_stripes_and_base_offsets() {
    assert_eq!(payload_by_lane(0, 0, 8, 4, 2).unwrap(), vec![4, 4]);
    assert_eq!(payload_by_lane(0, 3, 6, 4, 2).unwrap(), vec![2, 4]);
    assert_eq!(payload_by_lane(100, 103, 6, 4, 2).unwrap(), vec![2, 4]);
    assert_eq!(payload_by_lane(100, 108, 8, 4, 2).unwrap(), vec![4, 4]);
    assert_eq!(payload_by_lane(0, 0, 0, 4, 2).unwrap(), vec![0, 0]);
}

#[test]
fn accounting_matches_small_byte_oracle_without_per_stripe_children() {
    for count in [1usize, 2, 3, 16] {
        for stripe in [1u64, 4, 64] {
            for start in [0u64, 1, 3, 63, 64, 65] {
                for bytes in [1usize, 2, 7, 128, 1025] {
                    let mut expected = vec![0usize; count];
                    for offset in start..start + bytes as u64 {
                        expected[(offset / stripe) as usize % count] += 1;
                    }
                    assert_eq!(
                        payload_by_lane(100, 100 + start, bytes, stripe, count).unwrap(),
                        expected
                    );
                }
            }
        }
    }
    // Arithmetic stripe accounting must not allocate an object per stripe.
    let huge = payload_by_lane(0, 0, usize::MAX, 4, 16).unwrap();
    assert_eq!(huge.len(), 16);
    assert_eq!(
        huge.iter().map(|&v| v as u128).sum::<u128>(),
        usize::MAX as u128
    );
}

#[test]
fn checked_accounting_preserves_last_u64_byte_and_rejects_invalid_ranges() {
    assert_eq!(payload_by_lane(0, u64::MAX, 1, 4, 2).unwrap(), vec![0, 1]);
    assert_eq!(
        payload_by_lane(u64::MAX, u64::MAX, 1, 4, 2).unwrap(),
        vec![1, 0]
    );
    assert!(payload_by_lane(0, u64::MAX, 2, 4, 2).is_err());
    assert!(payload_by_lane(100, 99, 1, 4, 2).is_err());
    for (stripe, count) in [(0, 2), (3, 2), (4, 0), (4, 257)] {
        assert!(payload_by_lane(0, 0, 1, stripe, count).is_err());
    }
}
