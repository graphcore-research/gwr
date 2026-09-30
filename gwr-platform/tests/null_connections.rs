// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt::Write;

use gwr_engine::engine::Engine;
use gwr_platform::Platform;
use gwr_track::tracker::dev_null_tracker;

#[test]
fn null_connected_fabric_ports_run_without_unconnected_port_errors() {
    for kind in ["functional", "routed"] {
        let mut engine = Engine::new(&dev_null_tracker());
        let clock = engine.default_clock();
        let yaml = format!(
            "
memory_maps: []
fabrics:
  - name: fabric0
    kind: {kind}
    columns: 1
    rows: 2
    config: {{}}
connections:
  - connect: ['null', 'fabric.fabric0@(0,0)']
  - connect: ['fabric.fabric0@(0,1)', 'null']
",
        );
        let _platform = Platform::from_string(&engine, &clock, &yaml).unwrap();

        engine.run().unwrap();
    }
}

#[test]
fn null_connections_cover_the_former_hbm_fabric_ports() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let mut yaml = String::from(
        "memory_maps: []\nfabrics:\n  - name: fabric0\n    kind: functional\n    columns: 12\n    rows: 24\n    config: {}\nconnections:\n",
    );
    for column in 0..12 {
        for row in 0..24 {
            writeln!(
                yaml,
                "  - connect: ['null', 'fabric.fabric0@({column},{row})']"
            )
            .unwrap();
        }
    }
    let _platform = Platform::from_string(&engine, &clock, &yaml).unwrap();

    engine.run().unwrap();
}

#[test]
fn null_connected_cache_ports_run_without_unconnected_port_errors() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let _platform = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps: []
caches:
  - name: l1
    config: {}
connections:
  - connect: ['null', cache.l1.dev]
  - connect: [cache.l1.mem, 'null']
",
    )
    .unwrap();

    engine.run().unwrap();
}

#[test]
fn null_cannot_connect_to_a_processing_element() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let error = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm
    devices: []
processing_elements:
  - name: pe0
    memory_map: mm
    config: {}
connections:
  - connect: ['null', pe.pe0]
",
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("Null can only connect to a Fabric or Cache port")
    );
}

#[test]
fn null_connection_cannot_reuse_a_fabric_port() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let error = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps: []
fabrics:
  - name: fabric0
    kind: functional
    columns: 1
    rows: 2
    config: {}
connections:
  - connect: ['null', 'fabric.fabric0@(0,0)']
  - connect: ['null', 'fabric.fabric0@(0,0)']
",
    )
    .unwrap_err();

    assert!(error.to_string().contains("connected more than once"));
}
