// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::fabric::FabricPortSelection;
use gwr_models::roofline::accounting::{Operation, TransactionRun};
use gwr_platform::PreparedPlatform;
use gwr_platform::roofline_resources::ResourceKind;
use gwr_platform::types::PlatformConfig;

fn direct() -> PlatformConfig {
    yaml_serde::from_str("operation_classes: {binary: {operations: [add, mul, compare], arity: 2}}\nmemory_maps:\n  - name: mm\n    devices: [ram]\nprocessing_elements:\n  - name: pe\n    memory_map: mm\n    config: {compute: {engines: {vector: {capabilities: [{class: binary, ops_per_tick: 1}]}}}}\nmemories:\n  - name: ram\n    kind: hbm\n    base_address: 0\n    config: {capacity_bytes: 4096, bw_bytes_per_tick: 16, delay_ticks: 5}\nconnections:\n  - connect: [pe.pe, mem.ram]\n").unwrap()
}

fn run() -> [TransactionRun; 1] {
    [TransactionRun {
        payload_bytes: 64,
        count: 1,
        request_header_bytes: 8,
        response_header_bytes: 8,
    }]
}

#[test]
fn direct_routes_from_detailed_platform_charge_memory_once() {
    for reverse in [false, true] {
        let mut cfg = direct();
        if reverse {
            cfg.connections.as_mut().unwrap()[0].connect.reverse();
        }
        let prepared = PreparedPlatform::new(&cfg).unwrap();
        let routes = prepared.roofline_routes().unwrap();
        let resources = routes.resources();
        for op in [
            Operation::Read,
            Operation::WriteNonPosted,
            Operation::WritePosted,
        ] {
            let plan = routes
                .compile("pe", "ram", op)
                .unwrap()
                .plan(&run())
                .unwrap();
            assert_eq!(
                plan.resource_demands(),
                &[(
                    resources.index("ram", ResourceKind::MemoryPayload).unwrap(),
                    64
                )]
            );
            assert_eq!(plan.latency_ticks(), 5);
        }
    }
}

fn fabric(kind: &str, routing: &str) -> PlatformConfig {
    let mut cfg = direct();
    cfg.fabrics = yaml_serde::from_str(&format!("- name: net\n  kind: {kind}\n  columns: 2\n  rows: 2\n  config: {{fabric_ports_per_node: 1, ticks_per_hop: 2, ticks_overhead: 3, port_bits_per_tick: 64, routing: {routing}}}\n")).unwrap();
    cfg.connections = yaml_serde::from_str(
        "- connect: [pe.pe, 'fabric.net@(0,0)']\n- connect: [mem.ram, 'fabric.net@(1,1)']\n",
    )
    .unwrap();
    if kind == "routed" {
        let fabric = &mut cfg.fabrics.as_mut().unwrap()[0];
        fabric.config.port_bits_per_tick = None;
        let paths = gwr_models::fabric::paths::FabricPathConfig {
            bw_bits_per_tick: 64,
            ..gwr_models::fabric::paths::FabricPathConfig::default()
        };
        fabric.config.paths = Some(paths);
    }
    cfg
}

#[test]
fn column_first_response_is_not_the_reversed_request_path() {
    let cfg = fabric("routed", "column-first");
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    let routes = prepared.roofline_routes().unwrap();
    let plan = routes
        .compile("pe", "ram", Operation::Read)
        .unwrap()
        .plan(&run())
        .unwrap();
    for (from, to, bytes) in [
        ((0, 0), (1, 0), 8),
        ((1, 0), (1, 1), 8),
        ((1, 1), (0, 1), 72),
        ((0, 1), (0, 0), 72),
    ] {
        let index = routes
            .resources()
            .index("net", ResourceKind::FabricLink { from, to })
            .unwrap();
        assert!(plan.resource_demands().contains(&(index, bytes)));
    }
    let unused = routes
        .resources()
        .index(
            "net",
            ResourceKind::FabricLink {
                from: (1, 1),
                to: (1, 0),
            },
        )
        .unwrap();
    assert!(!plan.resource_demands().iter().any(|(r, _)| *r == unused));
    assert_eq!(plan.latency_ticks(), 19);
}

#[test]
fn row_first_and_functional_paths_have_distinct_demands_and_tails() {
    for (kind, tail, count) in [("routed", 19, 7), ("functional", 19, 5)] {
        let cfg = fabric(kind, "row-first");
        let prepared = PreparedPlatform::new(&cfg).unwrap();
        let routes = prepared.roofline_routes().unwrap();
        let plan = routes
            .compile("pe", "ram", Operation::Read)
            .unwrap()
            .plan(&run())
            .unwrap();
        assert_eq!(plan.latency_ticks(), tail);
        assert_eq!(plan.resource_demands().len(), count);
        if kind == "routed" {
            for (from, to, bytes) in [
                ((0, 0), (0, 1), 8),
                ((0, 1), (1, 1), 8),
                ((1, 1), (1, 0), 72),
                ((1, 0), (0, 0), 72),
            ] {
                let index = routes
                    .resources()
                    .index("net", ResourceKind::FabricLink { from, to })
                    .unwrap();
                assert!(plan.resource_demands().contains(&(index, bytes)));
            }
        }
        let posted = routes
            .compile("pe", "ram", Operation::WritePosted)
            .unwrap()
            .plan(&run())
            .unwrap();
        assert_eq!(posted.latency_ticks(), 12);
        assert_eq!(
            posted.resource_demands().len(),
            if kind == "routed" { 4 } else { 3 }
        );
    }
}

#[test]
fn invalid_routes_fail_before_execution() {
    for case in ["missing", "unmapped", "cache"] {
        let mut cfg = direct();
        match case {
            "missing" => cfg.connections = None,
            "unmapped" => cfg.memory_maps[0].devices.clear(),
            "cache" => {
                cfg.caches = yaml_serde::from_str("- name: cache\n  config: {}\n").unwrap();
                cfg.connections = yaml_serde::from_str(
                    "- connect: [pe.pe, cache.cache]\n- connect: [cache.cache, mem.ram]\n",
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let prepared = PreparedPlatform::new(&cfg).unwrap();
        let routes = prepared.roofline_routes().unwrap();
        assert!(
            routes.compile("pe", "ram", Operation::Read).is_err(),
            "{case}"
        );
        assert!(routes.compile("absent", "ram", Operation::Read).is_err());
        assert!(routes.compile("pe", "absent", Operation::Read).is_err());
    }
}

fn chained() -> PlatformConfig {
    let mut cfg = direct();
    cfg.fabrics = yaml_serde::from_str("- name: left\n  kind: functional\n  columns: 3\n  rows: 1\n  config: {fabric_ports_per_node: 1, ticks_per_hop: 2, ticks_overhead: 3, port_bits_per_tick: 64, port_selection: source-id-modulo}\n- name: right\n  kind: functional\n  columns: 3\n  rows: 1\n  config: {fabric_ports_per_node: 1, ticks_per_hop: 2, ticks_overhead: 3, port_bits_per_tick: 64, port_selection: source-id-modulo}\n").unwrap();
    cfg.connections = yaml_serde::from_str("- connect: [pe.pe, 'fabric.left@(0,0)']\n- connect: ['fabric.left@(1,0)', 'fabric.right@(0,0)']\n- connect: ['fabric.left@(2,0)', 'fabric.right@(1,0)']\n- connect: ['fabric.right@(2,0)', mem.ram]\n").unwrap();
    cfg
}

#[test]
fn chained_source_modulo_routes_swap_protocol_source_for_responses() {
    let cfg = chained();
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    assert_eq!(prepared.device_id("pe").unwrap().0, 0);
    assert_eq!(prepared.device_id("ram").unwrap().0, 1);
    let routes = prepared.roofline_routes().unwrap();
    let plan = routes
        .compile("pe", "ram", Operation::Read)
        .unwrap()
        .plan(&run())
        .unwrap();
    for (name, kind, bytes) in [
        ("left", ResourceKind::FabricEgress { port: 1 }, 8),
        ("right", ResourceKind::FabricIngress { port: 0 }, 8),
        ("right", ResourceKind::FabricEgress { port: 1 }, 72),
        ("left", ResourceKind::FabricIngress { port: 2 }, 72),
    ] {
        assert!(
            plan.resource_demands()
                .contains(&(routes.resources().index(name, kind).unwrap(), bytes))
        );
    }
    assert_eq!(plan.resource_demands().len(), 9);
    assert_eq!(plan.latency_ticks(), 29);
}

#[test]
fn multi_candidate_hash_is_rejected_only_for_directions_that_exist() {
    let mut cfg = chained();
    cfg.fabrics.as_mut().unwrap()[1].config.port_selection =
        Some(FabricPortSelection::DestinationAddressHash);
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    let routes = prepared.roofline_routes().unwrap();
    let error = routes.compile("pe", "ram", Operation::Read).unwrap_err();
    assert!(error.to_string().contains("transaction-ledger splitting"));
    assert!(
        routes
            .compile("pe", "ram", Operation::WriteNonPosted)
            .is_err()
    );
    let plan = routes
        .compile("pe", "ram", Operation::WritePosted)
        .unwrap()
        .plan(&run())
        .unwrap();
    assert_eq!(plan.latency_ticks(), 17);
    assert_eq!(plan.traffic().response_wire, 0);
    cfg.fabrics.as_mut().unwrap()[0].config.port_selection =
        Some(FabricPortSelection::DestinationAddressHash);
    assert!(
        PreparedPlatform::new(&cfg)
            .unwrap()
            .roofline_routes()
            .unwrap()
            .compile("pe", "ram", Operation::WritePosted)
            .is_err()
    );
}

#[test]
fn route_latency_arithmetic_is_checked() {
    for multiply in [true, false] {
        let mut cfg = fabric("functional", "column-first");
        let config = &mut cfg.fabrics.as_mut().unwrap()[0].config;
        if multiply {
            config.ticks_per_hop = Some(usize::MAX);
        } else {
            config.ticks_overhead = Some(usize::MAX);
        }
        let prepared = PreparedPlatform::new(&cfg).unwrap();
        assert!(
            prepared
                .roofline_routes()
                .unwrap()
                .compile("pe", "ram", Operation::Read)
                .is_err()
        );
    }
}

#[test]
fn routed_multi_attachment_requires_address_ledger_even_with_source_modulo() {
    let mut cfg = chained();
    let fabric = &mut cfg.fabrics.as_mut().unwrap()[0];
    fabric.kind = gwr_platform::types::FabricKind::Routed;
    let rate = fabric.config.port_bits_per_tick.take().unwrap();
    let paths = gwr_models::fabric::paths::FabricPathConfig {
        bw_bits_per_tick: rate,
        ..gwr_models::fabric::paths::FabricPathConfig::default()
    };
    fabric.config.paths = Some(paths);
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    let error = prepared
        .roofline_routes()
        .unwrap()
        .compile("pe", "ram", Operation::Read)
        .unwrap_err();
    assert!(error.to_string().contains("transaction-ledger splitting"));
}
