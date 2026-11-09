// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
#![cfg(feature = "functional")]
use gwr_models::memory::{FunctionalMemory, FunctionalValue as V};
use gwr_models::processing_element::tensors::dtype::DataType as D;

#[test]
fn every_dtype_preserves_its_declared_bits() {
    let memory = FunctionalMemory::default();
    for (i, dtype) in D::ALL.into_iter().enumerate() {
        let bits = if dtype.num_bits() == 64 {
            u64::MAX
        } else {
            (1u64 << dtype.num_bits()) - 1
        };
        let values = [
            V::from_bits(dtype, bits).unwrap(),
            V::from_bits(dtype, 0).unwrap(),
        ];
        memory.write_values(i as u64 * 32, dtype, &values).unwrap();
        assert_eq!(memory.read_values(i as u64 * 32, dtype, 2).unwrap(), values);
        memory.check_values(i as u64 * 32, dtype, &values).unwrap();
    }
}

#[test]
fn integer_values_and_casts_never_round_through_float() {
    for value in [
        i64::MIN,
        -9_007_199_254_740_993,
        -1,
        0,
        9_007_199_254_740_993,
        i64::MAX,
    ] {
        let v = V::from_i64(D::Int64, value).unwrap();
        assert_eq!(v.as_i64().unwrap(), value);
        assert_eq!(v.convert(D::Uint64).unwrap().bits(), value as u64);
    }
    for value in [0, 9_007_199_254_740_993, u64::MAX] {
        assert_eq!(
            V::from_u64(D::Uint64, value).unwrap().as_u64().unwrap(),
            value
        );
    }
    assert!(V::from_i64(D::Int4, 8).is_err());
    assert!(V::from_u64(D::Uint8, 256).is_err());
    assert_eq!(
        V::from_i64(D::Int64, 257)
            .unwrap()
            .convert(D::Int8)
            .unwrap()
            .as_i64()
            .unwrap(),
        1
    );
}

#[test]
fn fp16_bfloat_rounding_and_special_values() {
    for (dtype, one, tie, above) in [
        (D::Fp16, 0x3c00, 1. + 2f32.powi(-11), 1.000_488_4),
        (D::Bf16, 0x3f80, 1. + 2f32.powi(-8), 1.003_906_4),
    ] {
        assert_eq!(V::from_f32(dtype, 1.).unwrap().bits(), one);
        assert_eq!(V::from_f32(dtype, tie).unwrap().bits(), one);
        assert_eq!(V::from_f32(dtype, above).unwrap().bits(), one + 1);
        for value in [0., -0., f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                V::from_f32(dtype, value)
                    .unwrap()
                    .as_f32()
                    .unwrap()
                    .to_bits(),
                value.to_bits()
            );
        }
        assert!(
            V::from_f32(dtype, f32::NAN)
                .unwrap()
                .as_f32()
                .unwrap()
                .is_nan()
        );
    }
    assert_eq!(V::from_f32(D::Fp16, 2f32.powi(-24)).unwrap().bits(), 1);
    assert_eq!(V::from_f32(D::Fp16, 2f32.powi(-25)).unwrap().bits(), 0);
}

#[test]
fn packed_tail_loads_require_only_initialized_bits() {
    let memory = FunctionalMemory::default();
    memory
        .write_values(0, D::Int4, &[V::from_i64(D::Int4, -7).unwrap()])
        .unwrap();
    assert_eq!(
        memory.read_values(0, D::Int4, 1).unwrap()[0]
            .as_i64()
            .unwrap(),
        -7
    );
    assert!(memory.read_values(0, D::Int4, 2).is_err());
    assert!(memory.read_values(0, D::Uint8, 1).is_err());
    assert!(
        memory
            .write_values(u64::MAX, D::Uint64, &[V::from_u64(D::Uint64, 1).unwrap()])
            .is_err()
    );
    assert!(memory.read_values(u64::MAX, D::Uint64, 1).is_err());
}

#[test]
fn explicitly_named_e4m3fn_and_e2m1_encodings() {
    for (dtype, cases) in [
        (
            D::Fp8,
            vec![(0x01, 2f32.powi(-9)), (0x38, 1.), (0x7e, 448.), (0xb8, -1.)],
        ),
        (D::Fp4, vec![(1, 0.5), (2, 1.), (7, 6.), (10, -1.)]),
    ] {
        for (bits, value) in cases {
            assert_eq!(V::from_bits(dtype, bits).unwrap().as_f32().unwrap(), value);
            assert_eq!(V::from_f32(dtype, value).unwrap().bits(), bits);
        }
    }
    assert!(
        V::from_bits(D::Fp8, 0x7f)
            .unwrap()
            .as_f32()
            .unwrap()
            .is_nan()
    );
    assert_eq!(V::from_f32(D::Fp8, 1.0625).unwrap().bits(), 0x38);
    assert_eq!(V::from_f32(D::Fp8, f32::INFINITY).unwrap().bits(), 0x7e);
    assert_eq!(V::from_f32(D::Fp4, 1.25).unwrap().bits(), 2);
    assert_eq!(V::from_f32(D::Fp4, 100.).unwrap().bits(), 7);
    assert!(V::from_f32(D::Fp4, f32::NAN).is_err());
}

#[test]
fn bf16_subnormal_roundtrip_and_integer_single_rounding() {
    for bits in 0..65_536 {
        let value = V::from_bits(D::Bf16, bits).unwrap().as_f32().unwrap();
        if !value.is_nan() {
            assert_eq!(V::from_f32(D::Bf16, value).unwrap().bits(), bits);
        }
    }
    let x = (1u64 << 63) + (1 << 39) + 1;
    assert_eq!(
        V::from_u64(D::Uint64, x)
            .unwrap()
            .convert(D::Fp32)
            .unwrap()
            .bits(),
        0x5f00_0001
    );
    let x = (1u64 << 31) + (1 << 23) + 1;
    assert_eq!(
        V::from_u64(D::Uint64, x)
            .unwrap()
            .convert(D::Bf16)
            .unwrap()
            .bits(),
        0x4f01
    );
}

#[test]
fn integer_constructor_bounds_and_narrowing_cover_each_width() {
    for dtype in [D::Int4, D::Int8, D::Int16, D::Int32, D::Int64] {
        let min = -(1i128 << (dtype.num_bits() - 1));
        let max = (1i128 << (dtype.num_bits() - 1)) - 1;
        for value in [min as i64, -1, 0, max as i64] {
            assert_eq!(V::from_i64(dtype, value).unwrap().as_i64().unwrap(), value);
        }
        if dtype != D::Int64 {
            assert!(V::from_i64(dtype, (min - 1) as i64).is_err());
            assert!(V::from_i64(dtype, (max + 1) as i64).is_err());
            assert_eq!(
                V::from_i64(D::Int64, (max + 1) as i64)
                    .unwrap()
                    .convert(dtype)
                    .unwrap()
                    .as_i64()
                    .unwrap(),
                min as i64
            );
        }
    }
    for dtype in [D::Uint8, D::Uint16, D::Uint32, D::Uint64] {
        let max = u64::MAX >> (64 - dtype.num_bits());
        assert_eq!(V::from_u64(dtype, max).unwrap().as_u64().unwrap(), max);
        if dtype != D::Uint64 {
            assert!(V::from_u64(dtype, max + 1).is_err());
        }
    }
}

#[test]
fn small_float_codecs_roundtrip_all_finite_encodings() {
    for dtype in [D::Fp16, D::Bf16, D::Fp8, D::Fp4] {
        for bits in 0..(1u64 << dtype.num_bits()) {
            let value = V::from_bits(dtype, bits).unwrap().as_f32().unwrap();
            if !value.is_nan() {
                assert_eq!(
                    V::from_f32(dtype, value).unwrap().bits(),
                    bits,
                    "{dtype:?} {bits:#x}"
                );
            }
        }
    }
}

#[test]
fn packed_layout_is_little_endian_with_no_row_padding() {
    let memory = FunctionalMemory::default();
    memory
        .write_values(
            0,
            D::Int4,
            &[-8, -1, 0, 7].map(|value| V::from_i64(D::Int4, value).unwrap()),
        )
        .unwrap();
    assert_eq!(
        memory
            .read_values(0, D::Uint8, 2)
            .unwrap()
            .iter()
            .map(|value| value.bits())
            .collect::<Vec<_>>(),
        [0xf8, 0x70]
    );
    memory
        .write_values(
            8,
            D::Bool,
            &[true, false, true, false, false, false, false, true].map(V::from_bool),
        )
        .unwrap();
    assert_eq!(memory.read_values(8, D::Uint8, 1).unwrap()[0].bits(), 0x85);
    assert!(
        memory
            .write_values(20, D::Uint8, &[V::from_bool(true)])
            .is_err()
    );
    assert!(memory.read_values(20, D::Uint8, 1).is_err());
}

#[test]
fn float_integer_conversion_saturates_truncates_and_preserves_bool_semantics() {
    for (value, signed, unsigned) in [
        (f32::NAN, 0, 0),
        (f32::INFINITY, i64::MAX, u64::MAX),
        (f32::NEG_INFINITY, i64::MIN, 0),
        (-1.9, -1, 0),
        (1.9, 1, 1),
    ] {
        let value = V::from_f32(D::Fp32, value).unwrap();
        assert_eq!(value.convert(D::Int64).unwrap().as_i64().unwrap(), signed);
        assert_eq!(
            value.convert(D::Uint64).unwrap().as_u64().unwrap(),
            unsigned
        );
    }
    assert_eq!(
        V::from_f32(D::Fp32, -0.).unwrap().convert(D::Bool).unwrap(),
        V::from_bool(false)
    );
    assert_eq!(
        V::from_f32(D::Fp32, f32::NAN)
            .unwrap()
            .convert(D::Bool)
            .unwrap(),
        V::from_bool(true)
    );
}

#[test]
fn each_adjacent_fp8_and_fp4_midpoint_uses_ties_to_even() {
    for (dtype, max, sign) in [(D::Fp8, 0x7e, 0x80), (D::Fp4, 7, 8)] {
        for low in 0..max {
            let lower = V::from_bits(dtype, low).unwrap().as_f32().unwrap();
            let upper = V::from_bits(dtype, low + 1).unwrap().as_f32().unwrap();
            let midpoint = lower.midpoint(upper);
            let expected = if low & 1 == 0 { low } else { low + 1 };
            assert_eq!(V::from_f32(dtype, midpoint).unwrap().bits(), expected);
            assert_eq!(
                V::from_f32(dtype, -midpoint).unwrap().bits(),
                expected | sign
            );
            assert_eq!(
                V::from_f32(dtype, midpoint.next_down()).unwrap().bits(),
                low
            );
            assert_eq!(
                V::from_f32(dtype, midpoint.next_up()).unwrap().bits(),
                low + 1
            );
        }
    }
}
