// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::engine::Engine;
use gwr_engine::types::DeviceId;
use gwr_platform::PreparedPlatform;
use gwr_platform::types::PlatformConfig;
use gwr_track::tracker::dev_null_tracker;

fn config() -> PlatformConfig {
    yaml_serde::from_str(
        "operation_classes: {binary: {operations: [add, mul, compare], arity: 2}}\n\nmemory_maps:\n  - name: global\n    devices: [high, low]\nprocessing_elements:\n  - name: pe0\n    memory_map: global\n    config: {compute: {engines: {vector: {capabilities: [{class: binary, ops_per_tick: 1}]}}}}\nmemories:\n  - name: low\n    kind: hbm\n    base_address: 4096\n    config: {capacity_bytes: 4096}\n  - name: high\n    kind: hbm\n    base_address: 16384\n    config: {capacity_bytes: 4096}\n",
    )
    .unwrap()
}

#[test]
fn preflight_resolves_device_ids_and_address_maps_without_models() {
    let cfg = config();
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    assert_eq!(prepared.device_id("pe0").unwrap(), DeviceId(0));
    assert_eq!(prepared.device_id("low").unwrap(), DeviceId(1));
    assert_eq!(prepared.device_id("high").unwrap(), DeviceId(2));
    let map = prepared.memory_map("global").unwrap();
    assert_eq!(map.num_regions(), 2);
    assert_eq!(map.lookup(4095), None);
    assert_eq!(map.lookup(4096), Some((DeviceId(1), 0)));
    assert_eq!(map.lookup(8191), Some((DeviceId(1), 4095)));
    assert_eq!(map.lookup(8192), None);
    assert_eq!(map.lookup(16384), Some((DeviceId(2), 0)));
    assert_eq!(map.lookup(20479), Some((DeviceId(2), 4095)));
    assert_eq!(map.lookup(20480), None);
    assert!(prepared.device_id("absent").is_err());
    assert!(prepared.memory_map("absent").is_err());
}

#[test]
fn preflight_rejects_invalid_references_before_building() {
    let mut cfg = config();
    cfg.memory_maps[0].devices[0] = "absent".into();
    let error = PreparedPlatform::new(&cfg)
        .err()
        .expect("invalid reference");
    assert!(error.to_string().contains("absent"));
}

#[test]
fn preflight_preserves_the_last_physical_byte_in_address_maps() {
    let mut cfg = config();
    let memories = cfg.memories.as_mut().unwrap();
    memories[1].base_address = u64::MAX;
    memories[1].config.capacity_bytes = 1;
    for pe in cfg.processing_elements.iter_mut().flatten() {
        pe.config.lsu_access_bytes = Some(1);
    }
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    let map = prepared.memory_map("global").unwrap();
    assert_eq!(map.lookup(u64::MAX - 1), None);
    assert_eq!(map.lookup(u64::MAX), Some((DeviceId(2), 0)));
}

#[test]
fn platform_build_rejects_nested_registration_without_leaking_work() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let cfg = config();
    let transaction = engine.registration_transaction().unwrap();
    let result = PreparedPlatform::new(&cfg).unwrap().build(&engine, &clock);
    assert!(
        result.is_err(),
        "Platform must own its registration transaction"
    );
    drop(transaction);
    engine.run().unwrap();
    assert_eq!(clock.tick_now().tick(), 0);
}
