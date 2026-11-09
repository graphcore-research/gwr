// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Uncompiled Task1 RED draft; exclusive owner must run before implementation.
use gwr_platform::types::{MemoryServiceLanesSection, PlatformConfig};
use gwr_platform::yaml::platform_to_yaml_str;

fn source(fields: &str) -> String {
    format!(
        "memory_maps: [{{name: map, devices: [hbm]}}]\nmemories: [{{name: hbm, kind: hbm, base_address: 0, config: {{capacity_bytes: 4096, bw_bytes_per_tick: 8, delay_ticks: 3, arrival_aware_service: {{max_pending_requests: 16{fields}}}}}}}]\n"
    )
}

#[test]
fn optional_lane_schema_roundtrips_and_preserves_omission() {
    let omitted: PlatformConfig = yaml_serde::from_str(&source("")).unwrap();
    assert!(
        omitted.memories.as_ref().unwrap()[0]
            .config
            .arrival_aware_service
            .as_ref()
            .unwrap()
            .lanes
            .is_none()
    );
    assert!(!platform_to_yaml_str(&omitted).unwrap().contains("lanes:"));
    let config: PlatformConfig = yaml_serde::from_str(&source(
        ", lanes: {interleave_bytes: 4, bytes_per_tick: [4, 4]}",
    ))
    .unwrap();
    let fixture: PlatformConfig =
        yaml_serde::from_str(include_str!("fixtures/memory-service-lanes/two-lanes.yaml")).unwrap();
    assert_eq!(
        config.memories.as_ref().unwrap()[0].config,
        fixture.memories.as_ref().unwrap()[0].config
    );
    let lanes: &MemoryServiceLanesSection = config.memories.as_ref().unwrap()[0]
        .config
        .arrival_aware_service
        .as_ref()
        .unwrap()
        .lanes
        .as_ref()
        .unwrap();
    assert_eq!(lanes.interleave_bytes, 4);
    assert_eq!(lanes.bytes_per_tick, vec![4, 4]);
    let yaml = platform_to_yaml_str(&config).unwrap();
    let roundtrip: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
    assert_eq!(
        config.memories.as_ref().unwrap()[0].config,
        roundtrip.memories.as_ref().unwrap()[0].config
    );
}

#[test]
fn lane_wire_rejects_unknown_float_bool_and_wrong_shapes() {
    for fields in [
        "interleave_bytes: 4, bytes_per_tick: [4, 4], typo: true",
        "interleave_bytes: 4.0, bytes_per_tick: [4, 4]",
        "interleave_bytes: true, bytes_per_tick: [4, 4]",
        "interleave_bytes: 4, bytes_per_tick: [4.0, 4]",
        "interleave_bytes: 4, bytes_per_tick: [true, 4]",
        "interleave_bytes: 4, bytes_per_tick: 8",
        "interleave_bytes: 4",
        "bytes_per_tick: [4, 4]",
    ] {
        assert!(
            yaml_serde::from_str::<PlatformConfig>(&source(&format!(", lanes: {{{fields}}}")))
                .is_err(),
            "{fields}"
        );
    }
}

#[test]
fn lane_budgets_rates_and_actual_clock_are_validated() {
    let mut engine = gwr_engine::engine::Engine::default();
    let clock = engine.default_clock();
    for fields in [
        "interleave_bytes: 0, bytes_per_tick: [4, 4]",
        "interleave_bytes: 3, bytes_per_tick: [4, 4]",
        "interleave_bytes: 4, bytes_per_tick: []",
        "interleave_bytes: 4, bytes_per_tick: [0, 4]",
        "interleave_bytes: 4, bytes_per_tick: [5, 4]",
        "interleave_bytes: 4, bytes_per_tick: [18446744073709551615, 1]",
    ] {
        assert!(
            gwr_platform::Platform::from_string(
                &engine,
                &clock,
                &source(&format!(", lanes: {{{fields}}}"))
            )
            .is_err(),
            "{fields}"
        );
    }
    let rates = vec![1u64; 257];
    let fields = format!(", lanes: {{interleave_bytes: 4, bytes_per_tick: {rates:?}}}");
    let oversized = source(&fields).replace("bw_bytes_per_tick: 8", "bw_bytes_per_tick: 512");
    assert!(gwr_platform::Platform::from_string(&engine, &clock, &oversized).is_err());
    // Resource vector budget is independent of data storage partition count.
    let rates = vec![1u64; 16];
    let reduced = source(&format!(
        ", lanes: {{interleave_bytes: 4, bytes_per_tick: {rates:?}}}"
    ))
    .replace("capacity_bytes: 4096", "capacity_bytes: 1")
    .replace("bw_bytes_per_tick: 8", "bw_bytes_per_tick: 16");
    gwr_platform::Platform::from_string(&engine, &clock, &reduced).unwrap();
    let nondefault = engine.clock_ghz(2.0);
    assert!(
        gwr_platform::Platform::from_string(
            &engine,
            &nondefault,
            &source(", lanes: {interleave_bytes: 4, bytes_per_tick: [4, 4]}")
        )
        .is_err()
    );
    assert!(
        yaml_serde::from_str::<PlatformConfig>(
            &source(", lanes: {interleave_bytes: 4, bytes_per_tick: [4, 4]}")
                .replace("bw_bytes_per_tick: 8", "bw_bytes_per_tick: 8.5")
        )
        .is_err()
    );
}
