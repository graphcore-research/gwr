// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_components::connect_port;
use gwr_engine::engine::Engine;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_models::memory::cache::{Cache, CacheConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_access_gen::MemoryAccessGen;
use gwr_models::memory::memory_access_gen::random::Random;
use gwr_models::memory::{Memory, MemoryConfig};

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
    seed: u64,
    base_addr: u64,
    end_addr: u64,
    alignment_mask: u64,
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
    let data_generator = Box::new(Random::new(
        top,
        "random",
        seed,
        SRC_ADDR,
        base_addr,
        end_addr,
        alignment_mask,
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
fn basics() {
    // Test sweeping across the entire cache contents
    let num_reads = (CACHE_CAPACITY_BYTES / LINE_SIZE_BYTES) + 10;
    let aligment_mask = !0xf;
    let (mut engine, cache, memory, traffic_gen) = build_system(
        0x123,
        BASE_ADDRESS,
        BASE_ADDRESS + CACHE_CAPACITY_BYTES as u64,
        aligment_mask,
        num_reads,
    );

    run_simulation!(engine);

    let num_hits = cache.num_hits();
    let num_misses = cache.num_misses();
    println!("Accesses: {num_reads}, hits: {num_hits}, misses: {num_misses}");

    assert_eq!(num_reads, num_hits + num_misses);

    assert_eq!(num_misses * LINE_SIZE_BYTES, memory.bytes_read());

    assert_eq!(
        traffic_gen.payload_bytes_received(),
        num_reads * LINE_SIZE_BYTES
    );
}
