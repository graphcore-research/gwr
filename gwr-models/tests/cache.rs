// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_components::connect_port;
use gwr_components::test_helpers::expect_value;
use gwr_engine::engine::Engine;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::SimObject;
use gwr_models::build_model_harness;
use gwr_models::cache::{Cache, CacheConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::traits::{AccessMemory, CacheAccess, ReadMemory};
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{MemoryTxn, create_default_memory_map, create_read, create_write};
use gwr_track::entity::GetEntity;

const BASE_ADDRESS: u64 = 0x80000;
const DST_ADDR: u64 = BASE_ADDRESS;
const SRC_ADDR: u64 = BASE_ADDRESS + 0x1000;

const BW_BYTES_PER_TICK: usize = 8;
const LINE_SIZE_BYTES: usize = 32;
const NUM_SETS: usize = 1024;
const NUM_WAYS: usize = 4;

const ACCESS_SIZE_BYTES: usize = LINE_SIZE_BYTES;
// A realistic number of overhead bytes for a memory access (src/dst/control)
const OVERHEAD_SIZE_BYTES: usize = 16;
const CACHE_CAPACITY_BYTES: usize = NUM_SETS * NUM_WAYS * LINE_SIZE_BYTES;
const DELAY_TICKS: usize = 20;

struct TestMemory {}

impl ReadMemory for TestMemory {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn create_cache(engine: &mut Engine) -> Rc<Cache<MemoryAccess>> {
    let clock = engine.default_clock();
    Cache::new_and_register(engine, &clock, engine.top(), "cache", cache_config()).unwrap()
}

fn cache_config() -> CacheConfig {
    CacheConfig::new(
        LINE_SIZE_BYTES,
        BW_BYTES_PER_TICK,
        NUM_SETS,
        NUM_WAYS,
        DELAY_TICKS,
    )
}

/// Create a memory which is big enough to ensure the cache can't hold it all
fn create_and_connect_memory<T>(engine: &mut Engine, cache: &Rc<Cache<T>>) -> Rc<Memory<T>>
where
    T: SimObject + CacheAccess,
{
    let clock = engine.default_clock();
    let top = engine.top();

    let config = MemoryConfig::new(
        BASE_ADDRESS,
        CACHE_CAPACITY_BYTES * NUM_WAYS * 2,
        BW_BYTES_PER_TICK,
        DELAY_TICKS,
    );
    let memory = Memory::new_and_register(engine, &clock, top, "memory", config).unwrap();

    connect_port!(cache, mem_tx => memory, rx).unwrap();
    connect_port!(memory, tx => cache, mem_rx).unwrap();

    memory
}

/// Test the basics of the cache by driving/handling all of the ports manually
mod full_cache_harness {
    use gwr_track::entity::GetEntity;

    use super::*;

    build_model_harness! {
        harness CacheHarness<T: CacheAccess> {
            component: cache: Rc<Cache<T>>,
            rx ports: {
                DevRx<T> => dev_rx,
                MemRx<T> => mem_rx
            },
            tx ports: {
                DevTx<T> => dev_tx,
                MemTx<T> => mem_tx
            },
        }
    }

    #[test]
    fn cache_miss_fills_line_and_subsequent_read_hits() {
        let mut engine = start_test(file!());
        let cache = create_cache(&mut engine);
        let mut harness = CacheHarness::new(engine, cache.clone());
        let memory_delay_ticks = 10;
        let memory_map = Rc::new(create_default_memory_map());
        let dst_addr = DST_ADDR + 0x40;

        let read = create_read(
            cache.entity(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        let expected_fill = MemoryTxn::read_req(dst_addr)
            .with_src_addr(SRC_ADDR)
            .with_bytes(ACCESS_SIZE_BYTES);
        let response = MemoryTxn::read_rsp(dst_addr)
            .with_src_addr(SRC_ADDR)
            .with_bytes(ACCESS_SIZE_BYTES);

        let clock = harness.clock.clone();
        harness.run_steps([par!([
            seq! {
                send_dev_rx!(read.clone());
                expect_dev_tx!(response.clone());
                send_dev_rx!(read);
                expect_dev_tx!(response);
            },
            script! {
                ports { requests = MemTx, responses = MemRx }
                async {
                    let fill: MemoryAccess = expect_value(
                        &mut requests, &expected_fill, "memory fill request",
                    ).await?;
                    clock.wait_ticks(memory_delay_ticks)?.await;
                    responses.put(fill.to_response(&TestMemory {})?)?.await;
                    Ok(())
                }
            },
        ])]);

        assert_eq!(cache.payload_bytes_read(), 2 * ACCESS_SIZE_BYTES);
        assert_eq!(cache.payload_bytes_written(), 0);
        assert_eq!(cache.num_misses(), 1);
        assert_eq!(cache.num_hits(), 1);
        assert_eq!(cache.traffic_snapshot().fill_requests, 1);
    }

    #[test]
    fn repeated_reads_to_same_line_forward_only_one_memory_request() {
        let num_accesses = 100;
        let ticks_per_request = OVERHEAD_SIZE_BYTES.div_ceil(BW_BYTES_PER_TICK) as u64;

        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let cache = Cache::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "cache",
            cache_config().with_admission_limits(128, 64),
        )
        .unwrap();
        let memory_map = Rc::new(create_default_memory_map());

        let request = create_read(
            cache.entity(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            DST_ADDR,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        let expected = MemoryTxn::read_req(DST_ADDR)
            .with_src_addr(SRC_ADDR)
            .with_bytes(ACCESS_SIZE_BYTES);

        let mut harness = CacheHarness::new(engine, cache.clone());

        harness.run_steps([
            par!([
                seq!(vec![send_dev_rx!(request); num_accesses]),
                script! {
                    ports { requests = MemTx }
                    async {
                        let fill: MemoryAccess = expect_value(
                            &mut requests, &expected, "coalesced memory fill",
                        ).await?;
                        assert!(fill.cache_fill_token().is_some());
                        Ok(())
                    }
                },
            ]),
            expect_no_traffic!(&[Port::DevTx, Port::MemTx], ticks_per_request),
            expect_no_traffic!(&[Port::DevTx, Port::MemTx], DELAY_TICKS as u64),
        ]);

        // All accesses are to the same address, so only the first should be
        // passed through
        assert_eq!(cache.payload_bytes_read(), num_accesses * ACCESS_SIZE_BYTES);
        assert_eq!(cache.payload_bytes_written(), 0);
    }
}

/// Test a cache with a cache connected directly to a memory model
mod dev_cache_harness {
    use super::*;

    build_model_harness! {
        harness CacheDevHarness<T: CacheAccess> {
            component: cache: Rc<Cache<T>>,
            rx ports: {
                DevRx<T> => dev_rx
            },
            tx ports: {
                DevTx<T> => dev_tx
            },
        }
    }

    #[test]
    fn cache_plus_mem() {
        let mut engine = start_test(file!());
        let cache = create_cache(&mut engine);
        let memory = create_and_connect_memory(&mut engine, &cache);
        let mut harness = CacheDevHarness::<MemoryAccess>::new(engine, cache.clone());
        let memory_map = Rc::new(create_default_memory_map());

        let num_rereads = 10;
        let dst_addr = DST_ADDR + 0x40;
        let read = create_read(
            cache.entity(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        let mut steps = Vec::new();
        for _ in 0..=num_rereads {
            steps.push(send_dev_rx!(read.clone()));
            steps.push(expect_dev_tx!(
                MemoryTxn::read_rsp(dst_addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES),
            ));
        }

        harness.run_steps(steps);

        assert_eq!(
            cache.payload_bytes_read(),
            (num_rereads + 1) * ACCESS_SIZE_BYTES
        );
        assert_eq!(cache.payload_bytes_written(), 0);
        assert_eq!(cache.num_misses(), 1);
        assert_eq!(cache.num_hits(), num_rereads);

        assert_eq!(memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_written(), 0);
    }

    /// Ensure that the cache holds as many tags as it has ways
    #[test]
    fn cache_ways() {
        let mut engine = start_test(file!());
        let cache = create_cache(&mut engine);
        let memory = create_and_connect_memory(&mut engine, &cache);
        let mut harness = CacheDevHarness::<MemoryAccess>::new(engine, cache.clone());
        let memory_map = Rc::new(create_default_memory_map());

        let num_iterations = 10;
        let mut steps = Vec::new();
        for _ in 0..num_iterations {
            for i in 0..NUM_WAYS {
                let dst_addr = DST_ADDR + (i * CACHE_CAPACITY_BYTES / NUM_WAYS) as u64;
                let read = create_read(
                    cache.entity(),
                    &memory_map,
                    ACCESS_SIZE_BYTES,
                    dst_addr,
                    SRC_ADDR,
                    OVERHEAD_SIZE_BYTES,
                );
                steps.push(send_dev_rx!(read));
                steps.push(expect_dev_tx!(
                    MemoryTxn::read_rsp(dst_addr)
                        .with_src_addr(SRC_ADDR)
                        .with_bytes(ACCESS_SIZE_BYTES),
                ));
            }
        }

        harness.run_steps(steps);

        let num_accesses = num_iterations * NUM_WAYS;
        assert_eq!(cache.payload_bytes_read(), num_accesses * ACCESS_SIZE_BYTES);
        assert_eq!(cache.payload_bytes_written(), 0);
        assert_eq!(cache.num_misses(), NUM_WAYS);
        assert_eq!(cache.num_hits(), num_accesses - NUM_WAYS);

        assert_eq!(memory.bytes_read(), NUM_WAYS * ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_written(), 0);
    }

    /// Ensure that the cache can only hold as many tags as it has ways
    #[test]
    fn cache_ways_overflow() {
        let mut engine = start_test(file!());
        let cache = create_cache(&mut engine);
        let memory = create_and_connect_memory(&mut engine, &cache);
        let mut harness = CacheDevHarness::<MemoryAccess>::new(engine, cache.clone());
        let memory_map = Rc::new(create_default_memory_map());

        let num_iterations = 10;
        let mut steps = Vec::new();
        for _ in 0..num_iterations {
            for i in 0..NUM_WAYS + 1 {
                let dst_addr = DST_ADDR + (i * CACHE_CAPACITY_BYTES / NUM_WAYS) as u64;
                let read = create_read(
                    cache.entity(),
                    &memory_map,
                    ACCESS_SIZE_BYTES,
                    dst_addr,
                    SRC_ADDR,
                    OVERHEAD_SIZE_BYTES,
                );
                steps.push(send_dev_rx!(read));
                steps.push(expect_dev_tx!(
                    MemoryTxn::read_rsp(dst_addr)
                        .with_src_addr(SRC_ADDR)
                        .with_bytes(ACCESS_SIZE_BYTES),
                ));
            }
        }

        harness.run_steps(steps);

        let num_accesses = num_iterations * (NUM_WAYS + 1);
        assert_eq!(cache.payload_bytes_read(), num_accesses * ACCESS_SIZE_BYTES);
        assert_eq!(cache.payload_bytes_written(), 0);
        assert_eq!(cache.num_misses(), num_accesses);
        assert_eq!(cache.num_hits(), 0);
        assert_eq!(memory.bytes_read(), num_accesses * ACCESS_SIZE_BYTES);
    }

    /// Ensure that a write causes a cache line to be flushed
    #[test]
    fn cache_write_flushes_line() {
        let mut engine = start_test(file!());
        let cache = create_cache(&mut engine);
        let memory = create_and_connect_memory(&mut engine, &cache);
        let mut harness = CacheDevHarness::<MemoryAccess>::new(engine, cache.clone());
        let memory_map = Rc::new(create_default_memory_map());

        let num_rereads = 3;
        let dst_addr = DST_ADDR;
        let read = create_read(
            cache.entity(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        let write = create_write(
            cache.entity(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        let mut steps = Vec::new();
        for _ in 0..num_rereads {
            steps.push(send_dev_rx!(read.clone()));
            steps.push(expect_dev_tx!(
                MemoryTxn::read_rsp(dst_addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES),
            ));
        }

        steps.push(send_dev_rx!(write));

        for _ in 0..num_rereads {
            steps.push(send_dev_rx!(read.clone()));
            steps.push(expect_dev_tx!(
                MemoryTxn::read_rsp(dst_addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES),
            ));
        }

        harness.run_steps(steps);

        assert_eq!(
            cache.payload_bytes_read(),
            num_rereads * 2 * ACCESS_SIZE_BYTES
        );
        assert_eq!(cache.payload_bytes_written(), ACCESS_SIZE_BYTES);
        assert_eq!(cache.num_misses(), 2);
        assert_eq!(cache.num_hits(), (num_rereads * 2) - 2);

        // Should have been read once before and once after write
        assert_eq!(memory.bytes_read(), 2 * ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_written(), ACCESS_SIZE_BYTES);
    }
}
