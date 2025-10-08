// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::cmp::min;
use std::rc::Rc;

use tramway_components::connect_port;
use tramway_engine::engine::Engine;
use tramway_engine::run_simulation;
use tramway_engine::test_helpers::start_test;
use tramway_models::memory::cache::{Cache, CacheConfig};
use tramway_models::memory::memory_access::MemoryAccess;
use tramway_models::memory::memory_access_gen::MemoryAccessGen;
use tramway_models::memory::memory_access_gen::strided::Strided;
use tramway_models::memory::{Memory, MemoryConfig};

const BASE_ADDRESS: u64 = 0x80000;
const SRC_ADDR: u64 = BASE_ADDRESS - LINE_SIZE_BYTES as u64;

const BW_BYTES_PER_CYCLE: usize = 8;
const LINE_SIZE_BYTES: usize = 32;
const NUM_SETS: usize = 32;
const NUM_WAYS: usize = 4;
const CACHE_CAPACITY_BYTES: usize = NUM_SETS * NUM_WAYS * LINE_SIZE_BYTES;
const DELAY_TICKS: usize = 20;

const OVERHEAD_SIZE_BYTES: usize = 16;

/// Helper to build a cache and the device-side ports to drive it.
fn build_system(
    base_addr: u64,
    stride_bytes: u64,
    end_addr: u64,
    num_to_send: usize,
) -> (
    Engine,
    Rc<Cache<MemoryAccess>>,
    Rc<Memory<MemoryAccess>>,
    Rc<MemoryAccessGen<MemoryAccess>>,
) {
    let mut engine = start_test(file!());
    let spawner = engine.spawner();
    let clock = engine.default_clock();

    let top = engine.top();
    let data_generator = Box::new(Strided::new(
        top,
        "strided",
        SRC_ADDR,
        base_addr,
        end_addr,
        stride_bytes,
        OVERHEAD_SIZE_BYTES,
        LINE_SIZE_BYTES,
        num_to_send,
    ));

    let traffic_gen =
        MemoryAccessGen::new_and_register(&engine, top, "gen", data_generator).unwrap();

    let config = CacheConfig::new(
        LINE_SIZE_BYTES,
        BW_BYTES_PER_CYCLE,
        NUM_SETS,
        NUM_WAYS,
        DELAY_TICKS,
    );
    let cache = Cache::new_and_register(
        &engine,
        top,
        "cache",
        clock.clone(),
        spawner.clone(),
        config,
    )
    .unwrap();

    connect_port!(traffic_gen, tx => cache, dev_rx).unwrap();
    connect_port!(cache, dev_tx => traffic_gen, rx).unwrap();

    let config = MemoryConfig::new(
        BASE_ADDRESS,
        CACHE_CAPACITY_BYTES * 2,
        BW_BYTES_PER_CYCLE,
        DELAY_TICKS,
    );
    let memory = Memory::new_and_register(&engine, top, "memory", clock, spawner, config).unwrap();

    connect_port!(cache, mem_tx => memory, rx).unwrap();
    connect_port!(memory, tx => cache, mem_rx).unwrap();

    (engine, cache, memory, traffic_gen)
}

#[test]
fn full_sweep_lines() {
    // Test sweeping across the entire cache contents
    let num_reads = (CACHE_CAPACITY_BYTES / LINE_SIZE_BYTES) + 10;
    let (mut engine, cache, memory, traffic_gen) = build_system(
        BASE_ADDRESS,
        LINE_SIZE_BYTES as u64,
        BASE_ADDRESS + CACHE_CAPACITY_BYTES as u64,
        num_reads,
    );

    run_simulation!(engine);

    assert_eq!(cache.payload_bytes_read(), num_reads * LINE_SIZE_BYTES);
    assert_eq!(cache.payload_bytes_written(), 0);

    // Will sweep cache so every address accessed should miss at most once
    let cache_lines_accessed = min(num_reads, CACHE_CAPACITY_BYTES / LINE_SIZE_BYTES);
    assert_eq!(cache.num_misses(), cache_lines_accessed);
    assert_eq!(cache.num_hits(), num_reads - cache_lines_accessed);

    // Every cache line needs to be filled from memory
    assert_eq!(memory.bytes_read(), cache_lines_accessed * LINE_SIZE_BYTES);
    assert_eq!(memory.bytes_written(), 0);

    assert_eq!(
        traffic_gen.payload_bytes_received(),
        num_reads * LINE_SIZE_BYTES
    );
}

#[test]
fn full_sweep_words() {
    // Test sweeping consecutive word addresses
    let stride_bytes: usize = 4;
    let num_reads = (CACHE_CAPACITY_BYTES + 16) / stride_bytes;
    let (mut engine, cache, memory, traffic_gen) = build_system(
        BASE_ADDRESS,
        stride_bytes as u64,
        BASE_ADDRESS + CACHE_CAPACITY_BYTES as u64,
        num_reads,
    );

    run_simulation!(engine);

    assert_eq!(cache.payload_bytes_read(), num_reads * LINE_SIZE_BYTES);
    assert_eq!(cache.payload_bytes_written(), 0);

    // Will sweep cache so every address accessed should miss at most once
    let cache_lines_accessed = min(
        (num_reads * stride_bytes).div_ceil(LINE_SIZE_BYTES),
        NUM_SETS * NUM_WAYS,
    );
    assert_eq!(cache.num_misses(), cache_lines_accessed);
    assert_eq!(cache.num_hits(), num_reads - cache_lines_accessed);

    // Every cache line needs to be filled from memory
    assert_eq!(memory.bytes_read(), cache_lines_accessed * LINE_SIZE_BYTES);
    assert_eq!(memory.bytes_written(), 0);

    assert_eq!(
        traffic_gen.payload_bytes_received(),
        num_reads * LINE_SIZE_BYTES
    );
}

#[test]
fn all_misses() {
    // Test sweeping enough memory that the accesses cache will never be a hit
    let num_reads = (CACHE_CAPACITY_BYTES / LINE_SIZE_BYTES) * 10;
    let (mut engine, cache, memory, traffic_gen) = build_system(
        BASE_ADDRESS,
        LINE_SIZE_BYTES as u64,
        BASE_ADDRESS + CACHE_CAPACITY_BYTES as u64 + (CACHE_CAPACITY_BYTES / NUM_WAYS) as u64,
        num_reads,
    );

    run_simulation!(engine);

    assert_eq!(cache.payload_bytes_read(), num_reads * LINE_SIZE_BYTES);
    assert_eq!(cache.payload_bytes_written(), 0);

    assert_eq!(cache.num_misses(), num_reads);
    assert_eq!(cache.num_hits(), 0);
    assert_eq!(memory.bytes_read(), num_reads * LINE_SIZE_BYTES);
    assert_eq!(memory.bytes_written(), 0);

    assert_eq!(
        traffic_gen.payload_bytes_received(),
        num_reads * LINE_SIZE_BYTES
    );
}
