// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_platform::rates::TickScale;

#[test]
fn published_units_convert_to_logical_operations_and_directional_bandwidth() {
    let tick = TickScale::from_picoseconds(1000).unwrap();
    assert_eq!(
        tick.flops_per_tick(1_307_400_000_000_000.0, 2).unwrap(),
        653700.0
    );
    assert_eq!(tick.bytes_per_tick(5_300_000_000_000).unwrap(), 5300);
    assert_eq!(tick.bits_per_tick(800_000_000_000).unwrap(), 800);
    assert_eq!(tick.operations_per_tick(500_000_000.0).unwrap(), 0.5);
    let tick = TickScale::from_picoseconds(2500).unwrap();
    assert_eq!(tick.bytes_per_tick(400_000_000).unwrap(), 1);
}

#[test]
fn invalid_or_unrepresentable_rates_are_rejected_instead_of_rounded() {
    assert!(TickScale::from_picoseconds(0).is_err());
    let tick = TickScale::from_picoseconds(1000).unwrap();
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(tick.operations_per_tick(rate).is_err());
        assert!(tick.flops_per_tick(rate, 2).is_err());
    }
    assert!(tick.flops_per_tick(1.0, 0).is_err());
    for rate in [0, 1, 1_500_000_000] {
        assert!(tick.bytes_per_tick(rate).is_err());
        assert!(tick.bits_per_tick(rate).is_err());
    }
    let long_tick = TickScale::from_picoseconds(u64::MAX).unwrap();
    assert!(long_tick.bytes_per_tick(u64::MAX).is_err());
    assert!(long_tick.operations_per_tick(f64::MAX).is_err());
    assert!(tick.operations_per_tick(f64::from_bits(1)).is_err());
}
