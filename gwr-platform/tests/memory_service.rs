// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;
fn source(extra: &str) -> String {
    format!(
        "memory_maps: [{{name: map, devices: [hbm]}}]\nmemories: [{{name: hbm, kind: hbm, base_address: 0, config: {{capacity_bytes: 4096, bw_bytes_per_tick: 3350, delay_ticks: 3{extra}}}}}]\n"
    )
}
#[test]
fn optional_service_is_strict_roundtrippable_and_bounded() {
    let omitted: PlatformConfig = yaml_serde::from_str(&source("")).unwrap();
    assert!(
        omitted.memories.as_ref().unwrap()[0]
            .config
            .arrival_aware_service
            .is_none()
    );
    let config: PlatformConfig = yaml_serde::from_str(&source(
        ", arrival_aware_service: {max_pending_requests: 16}",
    ))
    .unwrap();
    let yaml = platform_to_yaml_str(&config).unwrap();
    let roundtrip: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
    assert_eq!(
        config.memories.as_ref().unwrap()[0].config,
        roundtrip.memories.as_ref().unwrap()[0].config
    );
    let mut engine = gwr_engine::engine::Engine::default();
    let clock = engine.default_clock();
    gwr_platform::Platform::from_string(&engine, &clock, &yaml).unwrap();
    for fields in [
        "",
        "max_pending_requests: 2, typo: 1",
        "max_pending_requests: 1.5",
    ] {
        assert!(
            yaml_serde::from_str::<PlatformConfig>(&source(&format!(
                ", arrival_aware_service: {{{fields}}}"
            )))
            .is_err()
        );
    }
    for budget in [0, 4097] {
        assert!(
            gwr_platform::Platform::from_string(
                &engine,
                &clock,
                &source(&format!(
                    ", arrival_aware_service: {{max_pending_requests: {budget}}}"
                ))
            )
            .is_err()
        );
    }
}

#[test]
fn fractional_native_rate_and_nondefault_clock_are_rejected_only_for_opt_in() {
    assert!(
        yaml_serde::from_str::<PlatformConfig>(
            &source(", arrival_aware_service: {max_pending_requests: 2}").replace("3350", "3350.5")
        )
        .is_err()
    );
    let mut engine = gwr_engine::engine::Engine::default();
    let clock = engine.clock_ghz(2.0);
    assert!(
        gwr_platform::Platform::from_string(
            &engine,
            &clock,
            &source(", arrival_aware_service: {max_pending_requests: 2}")
        )
        .is_err()
    );
    gwr_platform::Platform::from_string(&engine, &clock, &source("")).unwrap();
}
