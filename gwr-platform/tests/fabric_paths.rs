// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::engine::Engine;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;
fn yaml(kind: &str, extra: &str) -> String {
    format!(
        "operation_classes: {{}}\nmemory_maps: [{{name: local, devices: [m]}}]\nmemories: [{{name: m, kind: hbm, base_address: 0, config: {{capacity_bytes: 4096}}}}]\nconnections: [{{connect: [mem.m, 'fabric.mesh@(0,0).0']}}]\nfabrics:\n  - name: mesh\n    kind: {kind}\n    columns: 2\n    rows: 1\n    config:\n      fabric_ports_per_node: 2\n      paths:\n        count: 4\n        bw_bits_per_tick: 435\n        selection: {{policy: address-xor-fold-v1, address_shift: 6}}\n        queue_bytes: 8192\n        queue_packets: 256\n{extra}"
    )
}
#[test]
fn routed_path_schema_round_trips_without_port_bandwidth() {
    let cfg: PlatformConfig = yaml_serde::from_str(&yaml("routed", "")).unwrap();
    let out = platform_to_yaml_str(&cfg).unwrap();
    let _: PlatformConfig = yaml_serde::from_str(&out).unwrap();
    let mut engine = Engine::default();
    let c = engine.default_clock();
    Platform::from_string(&engine, &c, &out).unwrap();
}
#[test]
fn functional_paths_and_legacy_routed_rates_reject() {
    for input in [
        yaml("functional", ""),
        yaml("routed", "      port_bits_per_tick: 435\n"),
        yaml("routed", "      internal_link_bits_per_tick: 435\n"),
    ] {
        let mut e = Engine::default();
        let c = e.default_clock();
        assert!(Platform::from_string(&e, &c, &input).is_err());
    }
}

#[test]
fn reachable_fetches_must_fit_the_address_affinity_region() {
    let input=yaml("routed", "").replace("operation_classes: {}","operation_classes: {add: {operations: [add], arity: 2}}")
        .replace("connections: [{connect: [mem.m, 'fabric.mesh@(0,0).0']}]","connections: [{connect: [mem.m, 'fabric.mesh@(0,0).0']}, {connect: [pe.p, 'fabric.mesh@(1,0).0']}]");
    let input = format!(
        "{input}\nprocessing_elements:\n - name: p\n   memory_map: local\n   config:\n    lsu_access_bytes: 128\n    compute: {{engines: {{cpu: {{capabilities: [{{class: add, ops_per_tick: 1}}]}}}}}}\n"
    );
    let cfg: PlatformConfig = yaml_serde::from_str(&input).unwrap();
    assert!(
        cfg.validate().is_err(),
        "128-byte fetch cannot preserve 64-byte affinity"
    );
}

#[test]
fn preflight_counts_transport_headers_with_explicit_and_default_paths() {
    for (fetch, queue, explicit) in [(64, 64, true), (8192, 8192, false)] {
        let input=yaml("routed", "").replace("operation_classes: {}","operation_classes: {add: {operations: [add], arity: 2}}")
            .replace("capacity_bytes: 4096","capacity_bytes: 16384")
            .replace("queue_bytes: 8192",&format!("queue_bytes: {queue}"))
            .replace("connections: [{connect: [mem.m, 'fabric.mesh@(0,0).0']}]","connections: [{connect: [mem.m, 'fabric.mesh@(0,0).0']}, {connect: [pe.p, 'fabric.mesh@(1,0).0']}]");
        let input = format!(
            "{input}\nprocessing_elements:\n - name: p\n   memory_map: local\n   config:\n    lsu_access_bytes: {fetch}\n    overhead_size_bytes: 16\n    compute: {{engines: {{cpu: {{capabilities: [{{class: add, ops_per_tick: 1}}]}}}}}}\n"
        );
        let mut cfg: PlatformConfig = yaml_serde::from_str(&input).unwrap();
        if !explicit {
            cfg.fabrics.as_mut().unwrap()[0].config.paths = None;
        }
        assert!(
            cfg.validate().is_err(),
            "transport includes {fetch}+16 bytes; explicit={explicit}"
        );
    }
}
