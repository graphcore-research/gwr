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
fn builds_the_named_striped_address_space_example() {
    let source = include_str!("../examples/simple_pe_striped_mem.yaml");
    let cfg: PlatformConfig = yaml_serde::from_str(source).unwrap();
    let pe = &cfg.processing_elements.as_ref().unwrap()[0];
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    let map = prepared.pe_memory_map(pe).unwrap();
    let physical = prepared.memory_map("pe_memory_map").unwrap();
    assert_eq!(
        cfg.effective_processing_element_config(pe)
            .unwrap()
            .lsu_access_bytes,
        64
    );
    assert_eq!(physical.resolve(0x8040).unwrap().physical_address, 0x8040);
    let packet = map.resolve_access(0x8050, 16).unwrap();
    assert_eq!(packet.device, prepared.device_id("hbm1").unwrap());
    assert_eq!(packet.physical_address, 0x9010);
    assert!(map.resolve_access(0x8078, 16).is_err());

    let yaml = gwr_platform::yaml::platform_to_yaml_str(&cfg).unwrap();
    let round_trip: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
    assert_eq!(round_trip.address_spaces, cfg.address_spaces);

    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    gwr_platform::Platform::from_string(&engine, &clock, &yaml).unwrap();
}

#[test]
fn rejects_invalid_named_space_selections_and_stripe_banks() {
    let source = include_str!("../examples/simple_pe_striped_mem.yaml");
    for (old, new, expected) in [
        (
            "address_space: striped_space",
            "address_space: unknown",
            "Unknown address space",
        ),
        (
            "memory_map: pe_memory_map",
            "memory_map: unknown",
            "Unknown memory map",
        ),
        (
            "address_space: striped_space",
            "address_space: striped_space\n    memory_map: pe_memory_map",
            "exactly one",
        ),
        (
            "[hbm0, hbm1]",
            "[hbm1, hbm0]",
            "increasing physical address order",
        ),
        ("[hbm0, hbm1]", "[hbm0, hbm0]", "Duplicate memory"),
        ("[hbm0, hbm1]", "[hbm0, external]", "not in memory map"),
    ] {
        let cfg: PlatformConfig = yaml_serde::from_str(&source.replace(old, new)).unwrap();
        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn shared_mappings_and_reachability_follow_resolved_stripes() {
    let mut cfg: PlatformConfig =
        yaml_serde::from_str(include_str!("../examples/simple_pe_striped_mem.yaml")).unwrap();
    let primary = cfg.processing_elements.as_ref().unwrap()[0].clone();
    let mut other = primary.clone();
    other.name = "other".into();
    cfg.processing_elements
        .as_mut()
        .unwrap()
        .push(other.clone());
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    prepared.validate_shared_address_mappings().unwrap();
    let reachable = prepared.reachable_memories(&primary.name).unwrap();
    assert_eq!(reachable.len(), 2);
    assert_eq!(
        prepared
            .pe_memory_map(&primary)
            .unwrap()
            .reachable_ranges(&reachable)
            .next(),
        Some(0x8000..0xa000),
    );

    cfg.connections.as_mut().unwrap().pop();
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    let reachable = prepared.reachable_memories(&primary.name).unwrap();
    assert_eq!(reachable.len(), 1);
    assert!(
        prepared
            .pe_memory_map(&primary)
            .unwrap()
            .reachable_ranges(&reachable)
            .next()
            .is_none()
    );

    other.address_space = None;
    other.memory_map = "pe_memory_map".into();
    cfg.processing_elements.as_mut().unwrap()[1] = other;
    let prepared = PreparedPlatform::new(&cfg).unwrap();
    let error = prepared
        .validate_shared_address_mappings()
        .unwrap_err()
        .to_string();
    assert!(error.contains("incompatible address mappings"), "{error}");
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
