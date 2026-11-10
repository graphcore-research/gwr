// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::panic::catch_unwind;

use gwr_std::time::Duration;

#[test]
fn rejects_negative_and_nonfinite_nanoseconds() {
    for nanoseconds in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(Duration::try_new(0, nanoseconds).is_err(), "{nanoseconds}");
    }
}

#[test]
fn normalizes_whole_seconds_without_losing_fractional_nanoseconds() {
    let time = Duration::new(2, 5_000_000_000.25);
    assert_eq!(time.as_secs(), 7);
    assert_eq!(time.subsec_nanos(), 0.25);
}

#[test]
fn rejects_normalization_beyond_the_seconds_range() {
    assert!(Duration::try_new(u32::MAX, 1_000_000_000.0).is_err());
}

#[test]
fn floating_seconds_include_the_subsecond_component() {
    assert_eq!(Duration::new(1, 500_000_000.0).as_secs_f64(), 1.5);
    assert_eq!(Duration::new(0, 0.5).as_secs_f64(), 0.5e-9);
}

#[test]
fn checked_add_carries_fractional_nanoseconds() {
    let result = Duration::new(1, 999_999_999.5)
        .checked_add(Duration::new(0, 0.75))
        .unwrap();
    assert_eq!(result, Duration::new(2, 0.25));
}

#[test]
fn checked_sub_borrows_without_dropping_the_fraction() {
    let result = Duration::new(1, 0.25)
        .checked_sub(Duration::new(0, 0.5))
        .unwrap();
    assert_eq!(result, Duration::new(0, 999_999_999.75));
}

#[test]
fn checked_arithmetic_rejects_overflow_and_underflow() {
    assert!(Duration::MAX.checked_add(Duration::new(1, 0.0)).is_none());
    assert!(
        Duration::new(0, 0.25)
            .checked_sub(Duration::new(0, 0.5))
            .is_none()
    );
    assert!(Duration::MAX.checked_mul(2).is_none());
    assert!(
        Duration::new(1_431_655_765, 333_333_333.5)
            .checked_mul(3)
            .is_none()
    );
}

#[test]
fn saturating_arithmetic_clamps_at_the_type_boundaries() {
    assert_eq!(
        Duration::MAX.saturating_add(Duration::new(1, 0.0)),
        Duration::MAX
    );
    assert_eq!(
        Duration::new(0, 0.25).saturating_sub(Duration::new(0, 0.5)),
        Duration::default()
    );
    assert_eq!(Duration::MAX.saturating_mul(2), Duration::MAX);
}

#[test]
fn max_includes_representable_fractions_in_the_final_nanosecond() {
    assert!(Duration::new(u32::MAX, 999_999_999.5) <= Duration::MAX);
    assert!(Duration::MAX.subsec_nanos() < 1_000_000_000.0);
}

#[test]
fn constructors_preserve_units_and_whole_nanosecond_precision() {
    const MAX_SECONDS: Duration = Duration::from_secs(u32::MAX);
    for (direct, checked, seconds, nanoseconds) in [
        (Duration::new(1, 0.25), Duration::try_new(1, 0.25), 1, 0.25),
        (Duration::from_secs(5), Duration::try_from_secs(5), 5, 0.0),
        (
            MAX_SECONDS,
            Duration::try_from_secs(u64::from(u32::MAX)),
            u32::MAX,
            0.0,
        ),
        (
            Duration::from_millis(1_250),
            Duration::try_from_millis(1_250),
            1,
            250_000_000.0,
        ),
        (
            Duration::from_micros(1_000_125),
            Duration::try_from_micros(1_000_125),
            1,
            125_000.0,
        ),
        (
            Duration::from_nanos(9_007_199_254_740_993),
            Duration::try_from_nanos(9_007_199_254_740_993),
            9_007_199,
            254_740_993.0,
        ),
        (
            Duration::from_nanos_u128(9_007_199_254_740_993),
            Duration::try_from_nanos_u128(9_007_199_254_740_993),
            9_007_199,
            254_740_993.0,
        ),
        (
            Duration::from_secs_f64(1.5),
            Duration::try_from_secs_f64(1.5),
            1,
            500_000_000.0,
        ),
    ] {
        for time in [direct, checked.unwrap()] {
            assert_eq!(time.as_secs(), seconds);
            assert_eq!(time.subsec_nanos(), nanoseconds);
        }
    }
}

#[test]
fn constructors_reject_values_beyond_the_seconds_range() {
    assert!(Duration::try_from_secs(u64::from(u32::MAX) + 1).is_err());
    assert!(Duration::try_from_millis(u64::MAX).is_err());
    assert!(Duration::try_from_micros(u64::MAX).is_err());
    assert!(Duration::try_from_nanos(u64::MAX).is_err());
    assert!(Duration::try_from_nanos_u128(u128::MAX).is_err());
    assert!(Duration::try_from_secs_f64(f64::from(u32::MAX) + 1.0).is_err());
    for seconds in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(Duration::try_from_secs_f64(seconds).is_err());
    }
}

#[test]
fn normalizes_large_float_nanoseconds_at_the_seconds_boundary() {
    let nanoseconds = (u64::from(u32::MAX) + 1) * 1_000_000_000 - 512;
    let time = Duration::new(0, nanoseconds as f64);
    assert_eq!(time.as_secs(), u32::MAX);
    assert_eq!(time.subsec_nanos(), 999_999_488.0);
}

#[test]
fn zero_is_canonical_even_for_negative_zero_input() {
    for time in [
        Duration::default(),
        Duration::new(0, -0.0),
        Duration::from_secs_f64(-0.0),
    ] {
        assert_eq!(time, Duration::ZERO);
        assert_eq!(time.subsec_nanos().to_bits(), 0.0_f64.to_bits());
    }
}

#[test]
fn decimal_nanoseconds_round_trip_at_small_and_large_times() {
    for time in [
        Duration::ZERO,
        Duration::new(0, 0.125),
        Duration::new(0, f64::MIN_POSITIVE),
        Duration::new(0, f64::from_bits(1)),
        Duration::new(42, 123_456_789.25),
        Duration::new(9_007_199, 254_740_993.125),
        Duration::MAX,
    ] {
        let text = time.to_nanos_string();
        assert_eq!(Duration::from_nanos_str(&text).unwrap(), time, "{text}");
    }
    assert_eq!(
        Duration::new(9_007_199, 254_740_993.125).to_nanos_string(),
        "9007199254740993.125"
    );
}

#[test]
fn decimal_nanoseconds_reject_invalid_and_out_of_range_inputs() {
    for text in [
        "",
        "-1",
        "NaN",
        "inf",
        ".5",
        "1.",
        "1..2",
        "1e3",
        "1.2e3",
        " 1",
        "1ns",
        "4294967296000000000",
    ] {
        assert!(Duration::from_nanos_str(text).is_err(), "{text}");
    }
}

#[test]
fn fractional_intervals_and_whole_nanosecond_exports_are_distinct() {
    let time = Duration::new(1, 0.125);
    assert_eq!(time.as_nanos(), 1_000_000_000);
    assert_eq!(time.as_nanos_f64(), 1_000_000_000.125);
}

#[test]
fn display_selects_units_and_preserves_requested_precision() {
    for (time, text) in [
        (Duration::new(5, 0.0), "5.00s"),
        (Duration::new(1, 500_000_000.0), "1.50s"),
        (Duration::new(0, 500_000_000.0), "500.00ms"),
        (Duration::new(0, 500_000.0), "500.00us"),
        (Duration::new(0, 42.0), "42ns"),
        (Duration::new(0, 0.125), "0.125ns"),
        (Duration::ZERO, "0ns"),
        (Duration::new(1, 0.0), "1000.00ms"),
        (Duration::new(0, 1_000_000.0), "1000.00us"),
        (Duration::new(0, 1_000.0), "1000ns"),
    ] {
        assert_eq!(time.to_string(), text);
    }
    assert_eq!(format!("{:.0}", Duration::new(5, 0.0)), "5s");
    assert_eq!(
        format!("{:.4}", Duration::new(0, 500_000_000.0)),
        "500.0000ms"
    );
    assert_eq!(format!("{:.1}", Duration::new(0, 3_000.0)), "3.0us");
    assert_eq!(format!("{:.3}", Duration::new(0, 42.0)), "42ns");
}

#[test]
fn operators_preserve_fractional_arithmetic() {
    let first = Duration::new(1, 0.75);
    let second = Duration::new(0, 0.5);
    assert_eq!(first + second, Duration::new(1, 1.25));
    assert_eq!(first - second, Duration::new(1, 0.25));
    assert_eq!(second * 3, Duration::new(0, 1.5));
    assert_eq!(Duration::MAX - Duration::MAX, Duration::ZERO);
    assert_eq!(Duration::ZERO + Duration::ZERO, Duration::ZERO);
}

#[test]
fn multiplication_preserves_precision_and_normalizes_carry() {
    for (time, factor, expected) in [
        (Duration::new(1, 500_000_000.25), 2, Duration::new(3, 0.5)),
        (
            Duration::new(9_007_199, 254_740_993.125),
            2,
            Duration::new(18_014_398, 509_481_986.25),
        ),
        (
            Duration::new(0, 999_999_999.5),
            u32::MAX,
            Duration::new(4_294_967_292, 852_516_352.5),
        ),
        (
            Duration::new(0, 0.75),
            u32::MAX,
            Duration::new(3, 221_225_471.25),
        ),
        (Duration::MAX, 0, Duration::ZERO),
        (Duration::MAX, 1, Duration::MAX),
    ] {
        assert_eq!(time.checked_mul(factor), Some(expected));
        assert_eq!(time.saturating_mul(factor), expected);
    }
}

#[test]
#[should_panic(expected = "Duration addition overflow")]
fn addition_operator_rejects_overflow() {
    let _ = Duration::MAX + Duration::new(1, 0.0);
}

#[test]
#[should_panic(expected = "Duration subtraction underflow")]
fn subtraction_operator_rejects_underflow() {
    let _ = Duration::ZERO - Duration::new(0, 0.125);
}

#[test]
#[should_panic(expected = "Duration multiplication overflow")]
fn multiplication_operator_rejects_overflow() {
    let _ = Duration::MAX * 2;
}

#[test]
fn convenience_constructors_panic_on_invalid_inputs() {
    let constructors: &[fn() -> Duration] = &[
        || Duration::new(0, -1.0),
        || Duration::new(0, f64::NAN),
        || Duration::new(u32::MAX, 1_000_000_000.0),
        || Duration::from_millis(u64::MAX),
        || Duration::from_micros(u64::MAX),
        || Duration::from_nanos(u64::MAX),
        || Duration::from_nanos_u128(u128::MAX),
        || Duration::from_secs_f64(f64::INFINITY),
        || Duration::from_secs_f64(f64::from(u32::MAX) + 1.0),
    ];
    for (index, constructor) in constructors.iter().enumerate() {
        assert!(catch_unwind(constructor).is_err(), "constructor {index}");
    }
}
