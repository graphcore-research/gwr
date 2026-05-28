// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::cmp::min;
use std::rc::Rc;

use gwr_components::connect_port;
use gwr_engine::engine::Engine;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::time::clock::Clock;
use gwr_models::cache::{Cache, CacheConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_access_gen::MemoryAccessGen;
use gwr_models::memory::memory_access_gen::strided::Strided;
use gwr_models::memory::memory_map::DeviceId;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::create_default_memory_map;

const BASE_ADDRESS: u64 = 0x80000;
const SRC_ADDR: u64 = BASE_ADDRESS - LINE_SIZE_BYTES as u64;

const BW_BYTES_PER_CYCLE: usize = 8;
const LINE_SIZE_BYTES: usize = 32;
const NUM_SETS: usize = 32;
const NUM_WAYS: usize = 4;
const CACHE_CAPACITY_BYTES: usize = NUM_SETS * NUM_WAYS * LINE_SIZE_BYTES;
const DELAY_TICKS: usize = 20;

const OVERHEAD_SIZE_BYTES: usize = 16;

struct System {
    engine: Engine,
    clock: Clock,
    cache: Rc<Cache<MemoryAccess>>,
    memory: Rc<Memory<MemoryAccess>>,
    traffic_gen: Rc<MemoryAccessGen<MemoryAccess>>,
}

/// Helper to build a cache and the device-side ports to drive it.
fn build_system(
    base_addr: u64,
    stride_bytes: u64,
    end_addr: u64,
    num_to_send: usize,
    max_outstanding_requests: Option<usize>,
) -> System {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let memory_map = Rc::new(create_default_memory_map());

    let top = engine.top();
    let data_generator = Box::new(Strided::new(
        top,
        "strided",
        &memory_map,
        SRC_ADDR,
        base_addr,
        end_addr,
        stride_bytes,
        OVERHEAD_SIZE_BYTES,
        LINE_SIZE_BYTES,
        num_to_send,
    ));

    let traffic_gen =
        MemoryAccessGen::new_and_register(&engine, &clock, top, "gen", data_generator).unwrap();

    if max_outstanding_requests.is_some() {
        traffic_gen
            .set_max_outstanding_requests(max_outstanding_requests)
            .unwrap();
    }

    let config = CacheConfig::new(
        DeviceId(0),
        LINE_SIZE_BYTES,
        BW_BYTES_PER_CYCLE,
        NUM_SETS,
        NUM_WAYS,
        DELAY_TICKS,
        &memory_map,
    );
    let cache = Cache::new_and_register(&engine, &clock, top, "cache", config).unwrap();

    connect_port!(traffic_gen, tx => cache, dev_rx).unwrap();
    connect_port!(cache, dev_tx => traffic_gen, rx).unwrap();

    let config = MemoryConfig::new(
        BASE_ADDRESS,
        CACHE_CAPACITY_BYTES * 2,
        BW_BYTES_PER_CYCLE,
        DELAY_TICKS,
    );
    let memory = Memory::new_and_register(&engine, &clock, top, "memory", config).unwrap();

    connect_port!(cache, mem_tx => memory, rx).unwrap();
    connect_port!(memory, tx => cache, mem_rx).unwrap();

    System {
        engine,
        clock,
        cache,
        memory,
        traffic_gen,
    }
}

#[test]
fn full_sweep_lines() {
    // Test sweeping across the entire cache contents
    let num_reads = (CACHE_CAPACITY_BYTES / LINE_SIZE_BYTES) + 10;
    let mut system = build_system(
        BASE_ADDRESS,
        LINE_SIZE_BYTES as u64,
        BASE_ADDRESS + CACHE_CAPACITY_BYTES as u64,
        num_reads,
        None,
    );

    let engine = &mut system.engine;
    run_simulation!(engine);

    assert_eq!(
        system.cache.payload_bytes_read(),
        num_reads * LINE_SIZE_BYTES
    );
    assert_eq!(system.cache.payload_bytes_written(), 0);

    // Will sweep cache so every address accessed should miss at most once
    let cache_lines_accessed = min(num_reads, CACHE_CAPACITY_BYTES / LINE_SIZE_BYTES);
    assert_eq!(system.cache.num_misses(), cache_lines_accessed);
    assert_eq!(system.cache.num_hits(), num_reads - cache_lines_accessed);

    // Every cache line needs to be filled from memory
    assert_eq!(
        system.memory.bytes_read(),
        cache_lines_accessed * LINE_SIZE_BYTES
    );
    assert_eq!(system.memory.bytes_written(), 0);

    assert_eq!(
        system.traffic_gen.payload_bytes_received(),
        num_reads * LINE_SIZE_BYTES
    );
}

#[test]
fn full_sweep_words() {
    // Test sweeping consecutive word addresses
    let stride_bytes: usize = 4;
    let num_reads = (CACHE_CAPACITY_BYTES + 16) / stride_bytes;
    let mut system = build_system(
        BASE_ADDRESS,
        stride_bytes as u64,
        BASE_ADDRESS + CACHE_CAPACITY_BYTES as u64,
        num_reads,
        None,
    );

    let engine = &mut system.engine;
    run_simulation!(engine);

    assert_eq!(
        system.cache.payload_bytes_read(),
        num_reads * LINE_SIZE_BYTES
    );
    assert_eq!(system.cache.payload_bytes_written(), 0);

    // Will sweep cache so every address accessed should miss at most once
    let cache_lines_accessed = min(
        (num_reads * stride_bytes).div_ceil(LINE_SIZE_BYTES),
        NUM_SETS * NUM_WAYS,
    );
    assert_eq!(system.cache.num_misses(), cache_lines_accessed);
    assert_eq!(system.cache.num_hits(), num_reads - cache_lines_accessed);

    // Every cache line needs to be filled from memory
    assert_eq!(
        system.memory.bytes_read(),
        cache_lines_accessed * LINE_SIZE_BYTES
    );
    assert_eq!(system.memory.bytes_written(), 0);

    assert_eq!(
        system.traffic_gen.payload_bytes_received(),
        num_reads * LINE_SIZE_BYTES
    );
}

fn build_all_misses_system(num_reads: usize, max_outstanding_requests: Option<usize>) -> System {
    build_system(
        BASE_ADDRESS,
        LINE_SIZE_BYTES as u64,
        BASE_ADDRESS + CACHE_CAPACITY_BYTES as u64 + (CACHE_CAPACITY_BYTES / NUM_WAYS) as u64,
        num_reads,
        max_outstanding_requests,
    )
}

#[test]
fn all_misses() {
    // Test sweeping enough memory that the accesses cache will never be a hit.
    // Stick to max_outstanding_requests being 1 to ensure there are no
    // overlapping requests that end up being merged.
    let num_reads = (CACHE_CAPACITY_BYTES / LINE_SIZE_BYTES) * 10;
    let mut system = build_all_misses_system(num_reads, Some(1));
    system
        .traffic_gen
        .set_max_outstanding_requests(Some(1))
        .unwrap();

    let engine = &mut system.engine;
    run_simulation!(engine);

    assert_eq!(
        system.cache.payload_bytes_read(),
        num_reads * LINE_SIZE_BYTES
    );
    assert_eq!(system.cache.payload_bytes_written(), 0);

    assert_eq!(system.cache.num_misses(), num_reads);
    assert_eq!(system.cache.num_hits(), 0);
    assert_eq!(system.memory.bytes_read(), num_reads * LINE_SIZE_BYTES);
    assert_eq!(system.memory.bytes_written(), 0);

    assert_eq!(
        system.traffic_gen.payload_bytes_received(),
        num_reads * LINE_SIZE_BYTES
    );
}

#[test]
fn optional_outstanding_limit_backpressures_generator() {
    let num_reads = 4;

    let mut unlimited_system = build_all_misses_system(num_reads, None);
    let unlimited_engine = &mut unlimited_system.engine;
    run_simulation!(unlimited_engine);

    let mut limited_system = build_all_misses_system(num_reads, Some(1));
    let limited_engine = &mut limited_system.engine;
    run_simulation!(limited_engine);

    assert_eq!(
        unlimited_system.memory.bytes_read(),
        num_reads * LINE_SIZE_BYTES
    );
    assert_eq!(
        limited_system.memory.bytes_read(),
        num_reads * LINE_SIZE_BYTES
    );
    assert_eq!(
        unlimited_system.traffic_gen.payload_bytes_received(),
        num_reads * LINE_SIZE_BYTES
    );
    assert_eq!(
        limited_system.traffic_gen.payload_bytes_received(),
        num_reads * LINE_SIZE_BYTES
    );
    assert!(limited_system.clock.time_now_ns() > unlimited_system.clock.time_now_ns());
}
