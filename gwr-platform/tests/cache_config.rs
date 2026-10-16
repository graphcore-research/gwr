// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::build_model_harness;
use gwr_models::memory::cache::Cache;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::test_helpers::{MemoryTxn, create_default_memory_map, create_read};
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;
use gwr_track::entity::GetEntity;

const CONFIG: &str = r"
memory_maps:
  - name: all_hbm
    devices: [hbm0]
memories:
  - name: hbm0
    kind: hbm
    base_address: 0x8_0000
    config: {capacity_bytes: 1MiB, bw_bytes_per_tick: 32, delay_ticks: 2}
caches:
  - name: l2
    config:
      line_size_bytes: 128
      num_sets: 16
      num_ways: 2
      bw_bytes_per_tick: 32
      delay_ticks: 1
      max_inflight_parents: 4
      max_inflight_fills: 2
connections:
  - connect: [cache.l2.mem, mem.hbm0]
";

build_model_harness! {
    harness PlatformCacheHarness<T: gwr_models::memory::traits::CacheAccess> {
        component: cache: Rc<Cache<T>>,
        rx ports: { DevRx<T> => dev_rx },
        tx ports: { DevTx<T> => dev_tx },
    }
}

#[test]
fn cache_configuration_survives_yaml_roundtrip() {
    let config: PlatformConfig = yaml_serde::from_str(CONFIG).unwrap();
    let emitted = platform_to_yaml_str(&config).unwrap();
    let roundtrip: PlatformConfig = yaml_serde::from_str(&emitted).unwrap();
    assert_eq!(
        roundtrip.caches.as_ref().unwrap()[0].config,
        config.caches.as_ref().unwrap()[0].config
    );
}

#[test]
fn platform_line_configuration_controls_actual_fill_traffic() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_string(&engine, &clock, CONFIG).unwrap();
    let cache = platform.cache("l2").unwrap().clone();
    let memory = platform.memory("hbm0").unwrap().clone();
    let map = Rc::new(create_default_memory_map());
    let read = |address| create_read(cache.entity(), &map, 1, address, 0x81000, 16);
    let mut harness = PlatformCacheHarness::<MemoryAccess>::new(engine, cache.clone());
    harness.run_steps([
        send_dev_rx!(read(0x80001)),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80001).with_bytes(1)),
        send_dev_rx!(read(0x8001f)),
        expect_dev_tx!(MemoryTxn::read_rsp(0x8001f).with_bytes(1)),
        send_dev_rx!(read(0x80020)),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80020).with_bytes(1)),
    ]);
    assert_eq!(memory.bytes_read(), 128);
    let traffic = cache.traffic_snapshot();
    assert_eq!(traffic.logical_read_bytes, 3);
    assert_eq!(traffic.downstream_read_bytes, 128);
    assert_eq!(traffic.fill_requests, 1);
    assert_eq!(traffic.request_hits, 2);
    assert_eq!(traffic.request_misses, 1);
    let occupancy = cache.inflight_fills_snapshot();
    assert_eq!(occupancy.current, 0);
    assert_eq!(occupancy.peak, 1);
}

#[test]
fn invalid_line_and_inflight_limits_fail_during_platform_build() {
    for (from, to) in [
        ("line_size_bytes: 128", "line_size_bytes: 0"),
        ("max_inflight_parents: 4", "max_inflight_parents: 0"),
        ("max_inflight_fills: 2", "max_inflight_fills: 0"),
    ] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        assert!(
            Platform::from_string(&engine, &clock, &CONFIG.replace(from, to)).is_err(),
            "{to}"
        );
    }
}
