// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Global fill limits bound pending work and allow memory latency overlap.
use std::rc::Rc;

use gwr_components::connect_port;
use gwr_engine::engine::Engine;
use gwr_models::build_model_harness;
use gwr_models::memory::cache::{Cache, CacheConfig};
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{MemoryTxn, create_default_memory_map, create_read};
use gwr_track::entity::GetEntity;
build_model_harness! { harness AdmissionHarness<T: gwr_models::memory::traits::CacheAccess> {
    component: cache: Rc<Cache<T>>,
    rx ports: { DevRx<T> => dev_rx },
    tx ports: { DevTx<T> => dev_tx },
}}

#[test]
fn global_fill_limit_controls_overlap_without_changing_traffic() {
    let (one_ticks, one, one_traffic) = run_read(1, 256, false);
    let (two_ticks, two, two_traffic) = run_read(2, 256, false);
    assert_eq!(one.peak, 1);
    assert_eq!(two.peak, 2);
    assert_eq!(one.current, 0);
    assert_eq!(two.current, 0);
    assert!(
        two_ticks < one_ticks,
        "two fills should overlap memory latency"
    );
    for traffic in [one_traffic, two_traffic] {
        assert_eq!(traffic.logical_read_bytes, 256);
        assert_eq!(traffic.downstream_read_bytes, 256);
        assert_eq!(traffic.fill_requests, 2);
        assert_eq!(traffic.downstream_request_header_bytes, 32);
        assert_eq!(traffic.downstream_response_header_bytes, 32);
    }
}

#[test]
fn large_nested_requests_stream_through_global_fill_limits() {
    for limit in [1, 2] {
        let (_, snapshot, traffic) = run_read(limit, 4096, true);
        assert_eq!(snapshot.peak, limit);
        assert_eq!(snapshot.current, 0);
        assert_eq!(traffic.logical_read_bytes, 4096);
        assert_eq!(traffic.downstream_read_bytes, 4096);
        assert_eq!(traffic.fill_requests, 32);
    }
}

fn run_read(
    max_inflight_fills: usize,
    read_size_bytes: usize,
    use_nested_cache: bool,
) -> (
    u64,
    gwr_models::memory::cache::CacheInflightFills,
    gwr_models::memory::cache::CacheTraffic,
) {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let cache_config =
        CacheConfig::new(128, 4096, 1, 16, 1).with_admission_limits(1, max_inflight_fills);
    let cache =
        Cache::new_and_register(&engine, &clock, engine.top(), "cache", cache_config.clone())
            .unwrap();
    let memory = Memory::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "memory",
        MemoryConfig::new(0x80000, 65536, 4096, 100),
    )
    .unwrap();
    if use_nested_cache {
        let lower_cache =
            Cache::new_and_register(&engine, &clock, engine.top(), "lower", cache_config).unwrap();
        connect_port!(cache, mem_tx => lower_cache, dev_rx).unwrap();
        connect_port!(lower_cache, dev_tx => cache, mem_rx).unwrap();
        connect_port!(lower_cache, mem_tx => memory, rx).unwrap();
        connect_port!(memory, tx => lower_cache, mem_rx).unwrap();
    } else {
        connect_port!(cache, mem_tx => memory, rx).unwrap();
        connect_port!(memory, tx => cache, mem_rx).unwrap();
    }
    let memory_map = Rc::new(create_default_memory_map());
    let read_request = create_read(
        cache.entity(),
        &memory_map,
        read_size_bytes,
        0x80000,
        0x81000,
        16,
    );
    let mut harness = AdmissionHarness::new(engine, cache.clone());
    harness.run_steps([
        send_dev_rx!(read_request),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(read_size_bytes)),
    ]);
    (
        harness.clock.tick_now().tick(),
        cache.inflight_fills_snapshot(),
        cache.traffic_snapshot(),
    )
}
