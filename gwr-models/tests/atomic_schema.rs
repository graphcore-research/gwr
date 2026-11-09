// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_models::memory::atomic::*;
#[test]
fn omission_and_closed_integer_service_schema_have_no_inferred_vendor_rate() {
    let absent: AtomicPolicy = yaml_serde::from_str("{}").unwrap();
    assert_eq!(yaml_serde::to_string(&absent).unwrap(), "{}\n");
    let source = "service: {capacity: 2, bytes_per_tick: 8, delay_ticks: 0, bf16_pair_per_tick: 1, i32_fetch_add_per_tick: 1, lane_interleave_bytes: 4, lane_bytes_per_tick: [8]}";
    let valid: AtomicPolicy = yaml_serde::from_str(source).unwrap();
    valid.service.as_ref().unwrap().validate().unwrap();
    for bad in [
        source.replace("bf16_pair_per_tick: 1", "bf16_pair_per_tick: .inf"),
        source.replace("bytes_per_tick: 8", "bytes_per_tick: 18446744073709551616"),
        source.replace("capacity: 2", "capacity: 2, vendor_rate: 999"),
    ] {
        assert!(yaml_serde::from_str::<AtomicPolicy>(&bad).is_err());
    }
    let zero: AtomicPolicy =
        yaml_serde::from_str(&source.replace("bf16_pair_per_tick: 1", "bf16_pair_per_tick: 0"))
            .unwrap();
    assert!(zero.service.unwrap().validate().is_err());
    assert!(yaml_serde::from_str::<AtomicWord>("{device: 7, address: 4096, invocation: 9, generation: 1, operation: fp32_fadd, scope: agent, ordering: monotonic}").is_err());
}
