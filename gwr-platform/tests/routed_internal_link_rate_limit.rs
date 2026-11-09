// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_engine::engine::Engine;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;
fn yaml(kind: &str, columns: usize) -> String {
    format!(
        r"
memory_maps: [{{name: map, devices: [m]}}]
memories: [{{name: m, kind: hbm, base_address: 0, config: {{capacity_bytes: 4096}}}}]
connections: [{{connect: [mem.m, 'fabric.f@(0,0).0']}}]
fabrics:
 - name: f
   kind: {kind}
   columns: {columns}
   rows: 1
   config:
    fabric_ports_per_node: 2
    ticks_per_hop: 1
    paths:
     count: 1
     bw_bits_per_tick: 7
     selection: {{policy: address-xor-fold-v1, address_shift: 6}}
     queue_bytes: 64
     queue_packets: 8
"
    )
}
fn error(input: &str) -> String {
    let mut e = Engine::default();
    let c = e.default_clock();
    match Platform::from_string(&e, &c, input) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected invalid config"),
    }
}
#[test]
fn route_rate_round_trip_and_single_node_delivery() {
    for columns in [1, 2] {
        let input = yaml("routed", columns);
        let cfg: PlatformConfig = yaml_serde::from_str(&input).unwrap();
        let out = platform_to_yaml_str(&cfg).unwrap();
        let parsed: PlatformConfig = yaml_serde::from_str(&out).unwrap();
        assert_eq!(
            cfg.fabrics.unwrap()[0].config.paths,
            parsed.fabrics.unwrap()[0].config.paths
        );
        let mut e = Engine::default();
        let c = e.default_clock();
        Platform::from_string(&e, &c, &out).unwrap();
    }
}
#[test]
fn route_parameters_and_closed_schema_reject_invalid_inputs() {
    let input = yaml("routed", 2);
    for (old, new) in [
        ("bw_bits_per_tick: 7", "bw_bits_per_tick: 0"),
        ("count: 1", "count: 0"),
        ("count: 1", "count: 3"),
        ("queue_bytes: 64", "queue_bytes: 0"),
        ("queue_packets: 8", "queue_packets: 0"),
        ("address_shift: 6", "address_shift: 64"),
    ] {
        assert!(
            yaml_serde::from_str::<PlatformConfig>(&input.replace(old, new))
                .unwrap()
                .validate()
                .is_err()
        );
    }
    for value in ["0.5", "-1", "true"] {
        assert!(
            yaml_serde::from_str::<PlatformConfig>(
                &input.replace("bw_bits_per_tick: 7", &format!("bw_bits_per_tick: {value}"))
            )
            .is_err()
        );
    }
    assert!(
        yaml_serde::from_str::<PlatformConfig>(
            &input.replace("queue_packets: 8", "queue_packets: 8\n     unexpected: 1")
        )
        .is_err()
    );
}
#[test]
fn kind_specific_fields_and_zero_hop_delay_reject() {
    assert!(error(&yaml("functional", 2)).contains("paths require routed"));
    assert!(
        error(&yaml("routed", 2).replace("ticks_per_hop: 1", "ticks_per_hop: 0"))
            .contains("hop delay must be positive")
    );
    for field in ["port_bits_per_tick", "internal_link_bits_per_tick"] {
        assert!(
            error(
                &yaml("routed", 2).replace("    paths:", &format!("    {field}: 64\n    paths:"))
            )
            .contains(if field == "port_bits_per_tick" {
                "routed fabrics use paths.bw_bits_per_tick"
            } else {
                "unknown field"
            })
        );
    }
}
