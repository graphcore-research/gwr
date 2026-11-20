// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::engine::Engine;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;

#[test]
fn omitted_none_manual_yaml_shape_and_positive_roundtrip() {
    let baseline = yaml("routed", 2, "");
    let cfg: PlatformConfig = yaml_serde::from_str(&baseline).unwrap();
    let before = platform_to_yaml_str(&cfg).unwrap();
    assert!(!before.contains("internal_link_bits_per_tick"));
    let _: PlatformConfig = yaml_serde::from_str(&before).unwrap();
    assert_eq!(
        cfg.fabrics.as_ref().unwrap()[0]
            .config
            .internal_link_bits_per_tick,
        None
    );
    let mut config_with_rate_limit: PlatformConfig = yaml_serde::from_str(&baseline).unwrap();
    config_with_rate_limit.fabrics.as_mut().unwrap()[0]
        .config
        .internal_link_bits_per_tick = Some(7);
    let emitted = platform_to_yaml_str(&config_with_rate_limit).unwrap();
    let mut parsed: PlatformConfig = yaml_serde::from_str(&emitted).unwrap();
    assert_eq!(
        parsed.fabrics.as_ref().unwrap()[0]
            .config
            .internal_link_bits_per_tick,
        Some(7)
    );
    parsed.fabrics.as_mut().unwrap()[0]
        .config
        .internal_link_bits_per_tick = None;
    assert_eq!(platform_to_yaml_str(&parsed).unwrap(), before);
    let mut engine = Engine::default();
    Platform::from_string(&mut engine, &baseline).unwrap();
}

#[test]
fn schema_types_and_closed_keys_reject() {
    for field in [
        "internal_link_bits_per_tick: 0.5",
        "internal_link_bits_per_tick: -1",
        "internal_link_bits_per_tick: true",
        "internal_link_bits_per_tick_extra: 7",
    ] {
        assert!(
            yaml_serde::from_str::<PlatformConfig>(&yaml("routed", 2, &format!("      {field}\n")))
                .is_err(),
            "{field}"
        );
    }
}

#[test]
fn single_node_routed_fabric_accepts_unused_link_rate_limit() {
    let mut engine = Engine::default();
    Platform::from_string(
        &mut engine,
        &yaml("routed", 1, "      internal_link_bits_per_tick: 7\n"),
    )
    .unwrap();
}

#[test]
fn zero_hop_delay_is_rejected_for_all_fabric_kinds() {
    for kind in ["functional", "routed"] {
        let input = yaml(kind, 2, "").replace("ticks_per_hop: 1", "ticks_per_hop: 0");
        assert!(build_error(&input).contains("hop delay must be positive"));
    }
}

#[test]
fn invalid_opt_in_rejects_with_intended_categories() {
    // Valid legacy baseline first, including the one-node geometry.
    for (kind, columns) in [("functional", 2), ("routed", 1)] {
        let mut e = Engine::default();
        Platform::from_string(&mut e, &yaml(kind, columns, "")).unwrap();
    }
    assert!(
        build_error(&yaml("routed", 2, "      internal_link_bits_per_tick: 0\n"))
            .contains("rate must be positive")
    );
    assert!(
        build_error(&yaml(
            "functional",
            2,
            "      internal_link_bits_per_tick: 7\n"
        ))
        .contains("requires routed fabric")
    );
}

fn yaml(kind: &str, columns: usize, policy: &str) -> String {
    format!(
        r"clocks:
  - name: default
    frequency_mhz: 1000
default_clock: default

memory_maps:
  - name: map
    devices:
      - hbm0
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
      capacity_bytes: 4096
      bw_bytes_per_tick: 32
      delay_ticks: 0
connections:
  - connect:
      - mem.hbm0
      - 'fabric.f@(0,0).0'
fabrics:
  - name: f
    kind: {kind}
    columns: {columns}
    rows: 1
    config:
      fabric_ports_per_node: 2
      ticks_per_hop: 1
      ticks_overhead: 0
      rx_buffer_bytes: 64
      tx_buffer_bytes: 64
      port_bits_per_tick: 64
{policy}"
    )
}

fn build_error(input: &str) -> String {
    let mut engine = Engine::default();
    match Platform::from_string(&mut engine, input) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected invalid configuration"),
    }
}
