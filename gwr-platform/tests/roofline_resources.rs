// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_platform::PreparedPlatform;
use gwr_platform::roofline_resources::{CapacityUnit, ResourceKind};
use gwr_platform::types::PlatformConfig;

fn config() -> PlatformConfig {
    yaml_serde::from_str("operation_classes: {binary: {operations: [add, mul, compare], arity: 2}}\nmemory_maps:\n  - name: mm\n    devices: [ram]\nprocessing_elements:\n  - name: pe\n    memory_map: mm\n    config: {compute: {engines: {vector: {capabilities: [{class: binary, ops_per_tick: 1}]}}}}\ncaches:\n  - name: cache\n    config: {bw_bytes_per_tick: 7}\nmemories:\n  - name: ram\n    kind: hbm\n    base_address: 0\n    config: {capacity_bytes: 4096, bw_bytes_per_tick: 16}\n").unwrap()
}

#[test]
fn configured_capacities_preserve_units_and_independent_handlers() {
    let cfg = config();
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    let resources = prepared.roofline_resources().unwrap();
    assert_eq!(resources.resources().len(), 4);
    for (component, kind, expected, unit) in [
        (
            "pe",
            ResourceKind::PeCompute,
            1.,
            CapacityUnit::ServiceTicksPerTick,
        ),
        (
            "cache",
            ResourceKind::CacheRequest,
            7.,
            CapacityUnit::BytesPerTick,
        ),
        (
            "cache",
            ResourceKind::CacheMemoryResponse,
            7.,
            CapacityUnit::BytesPerTick,
        ),
        (
            "ram",
            ResourceKind::MemoryPayload,
            16.,
            CapacityUnit::BytesPerTick,
        ),
    ] {
        let index = resources.index(component, kind).unwrap();
        assert_eq!(resources.capacities()[index], expected);
        assert_eq!(resources.resources()[index].unit(), unit);
    }
    assert_ne!(
        resources.index("cache", ResourceKind::CacheRequest),
        resources.index("cache", ResourceKind::CacheMemoryResponse)
    );
    assert_eq!(resources, prepared.roofline_resources().unwrap());
}

fn fabric_config(kind: &str, columns: usize, rows: usize) -> PlatformConfig {
    let mut cfg: PlatformConfig = yaml_serde::from_str(&format!("memory_maps: []\nfabrics:\n  - name: net\n    kind: {kind}\n    columns: {columns}\n    rows: {rows}\n    config: {{fabric_ports_per_node: 1, port_bits_per_tick: 3}}\n")).unwrap();
    if kind == "routed" {
        let fabric = &mut cfg.fabrics.as_mut().unwrap()[0];
        fabric.config.port_bits_per_tick = None;
        let paths = gwr_models::fabric::paths::FabricPathConfig {
            bw_bits_per_tick: 3,
            ..gwr_models::fabric::paths::FabricPathConfig::default()
        };
        fabric.config.paths = Some(paths);
    }
    cfg
}

#[test]
fn fabric_ports_keep_fractional_rates_and_routed_links_are_directed_mesh_edges() {
    for (kind, count) in [("functional", 8), ("routed", 12)] {
        let cfg = fabric_config(kind, 2, 2);
        let catalog = PreparedPlatform::new(&cfg)
            .unwrap()
            .roofline_resources()
            .unwrap();
        assert_eq!(catalog.resources().len(), count);
        assert!(catalog.capacities().iter().all(|&rate| rate == 0.375));
        for port in 0..4 {
            let ingress = catalog.index("net", ResourceKind::FabricIngress { port });
            let egress = catalog
                .index("net", ResourceKind::FabricEgress { port })
                .unwrap();
            if kind == "functional" {
                assert_ne!(ingress.unwrap(), egress);
            } else {
                assert!(ingress.is_none());
            }
        }
        let link = |from, to| catalog.index("net", ResourceKind::FabricLink { from, to });
        if kind == "routed" {
            assert_ne!(link((0, 0), (1, 0)).unwrap(), link((1, 0), (0, 0)).unwrap());
            assert_ne!(link((0, 0), (0, 1)).unwrap(), link((0, 1), (0, 0)).unwrap());
        } else {
            assert!(link((0, 0), (1, 0)).is_none());
        }
        assert!(link((0, 0), (1, 1)).is_none());
        assert!(link((1, 0), (2, 0)).is_none());
    }
    let mut cfg = fabric_config("routed", 1, 1);
    cfg.fabrics.as_mut().unwrap()[0]
        .config
        .fabric_ports_per_node = Some(2);
    assert_eq!(
        PreparedPlatform::new(&cfg)
            .unwrap()
            .roofline_resources()
            .unwrap()
            .resources()
            .len(),
        2
    );
}

#[test]
fn effective_defaults_supply_analysis_capacities() {
    let mut cfg = config();
    cfg.memories.as_mut().unwrap()[0].config.bw_bytes_per_tick = None;
    cfg.caches.as_mut().unwrap()[0].config.bw_bytes_per_tick = None;
    let catalog = PreparedPlatform::new(&cfg)
        .unwrap()
        .roofline_resources()
        .unwrap();
    for (name, kind) in [
        ("ram", ResourceKind::MemoryPayload),
        ("cache", ResourceKind::CacheRequest),
        ("cache", ResourceKind::CacheMemoryResponse),
    ] {
        let index = catalog.index(name, kind).unwrap();
        assert_eq!(catalog.capacities()[index], 32.);
    }
}
