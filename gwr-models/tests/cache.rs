// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
use std::collections::HashMap;
use std::rc::Rc;

use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat};
use gwr_engine::engine::Engine;
use gwr_engine::port::OutPort;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::SimObject;
use gwr_models::build_model_harness;
use gwr_models::memory::cache::{Cache, CacheConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::traits::{AccessMemory, ReadMemory};
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{MemoryTxn, create_default_memory_map, create_read, create_write};
use gwr_track::entity::GetEntity;

const BASE_ADDRESS: u64 = 0x80000;
const DST_ADDR: u64 = BASE_ADDRESS;
const SRC_ADDR: u64 = BASE_ADDRESS + 0x1000;

const BW_BYTES_PER_CYCLE: usize = 8;
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

fn cache_config() -> CacheConfig {
    CacheConfig::new(
        LINE_SIZE_BYTES,
        BW_BYTES_PER_CYCLE,
        NUM_SETS,
        NUM_WAYS,
        DELAY_TICKS,
    )
}

fn create_cache(engine: &mut Engine) -> Rc<Cache<MemoryAccess>> {
    let clock = engine.default_clock();
    Cache::new_and_register(engine, &clock, engine.top(), "cache", cache_config()).unwrap()
}

/// Create a memory which is big enough to ensure the cache can't hold it all
fn create_and_connect_memory<T>(engine: &mut Engine, cache: &Rc<Cache<T>>) -> Rc<Memory<T>>
where
    T: SimObject + AccessMemory,
{
    let clock = engine.default_clock();
    let top = engine.top();

    let config = MemoryConfig::new(
        BASE_ADDRESS,
        CACHE_CAPACITY_BYTES * NUM_WAYS * 2,
        BW_BYTES_PER_CYCLE,
        DELAY_TICKS,
    );
    let memory = Memory::new_and_register(engine, &clock, top, "memory", config).unwrap();

    connect_port!(cache, mem_tx => memory, rx).unwrap();
    connect_port!(memory, tx => cache, mem_rx).unwrap();

    memory
}

#[test]
fn cache_dev_read_goes_to_mem() {
    let num_accesses = 100;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let memory_map = Rc::new(create_default_memory_map());

    let cache = create_cache(&mut engine);
    let top = engine.top();
    let source = Source::new_and_register(&engine, top, "source", None).unwrap();
    let to_put = create_read(
        source.entity(),
        &memory_map,
        ACCESS_SIZE_BYTES,
        DST_ADDR,
        SRC_ADDR,
        OVERHEAD_SIZE_BYTES,
    );
    source.set_generator(option_box_repeat!(to_put ; num_accesses));

    let dev_req_sink = Sink::new_and_register(&engine, &clock, top, "dev_req_sink").unwrap();
    let mem_req_sink = Sink::new_and_register(&engine, &clock, top, "mem_req_sink").unwrap();

    connect_port!(source, tx => cache, dev_rx).unwrap();
    connect_port!(cache, dev_tx => dev_req_sink, rx).unwrap();
    connect_port!(cache, mem_tx => mem_req_sink, rx).unwrap();

    // Even though we are not driving it we need to connect it.
    let mut mem_rx_driver = OutPort::new(top, "mem_rx_driver");
    mem_rx_driver.connect(cache.port_mem_rx()).unwrap();

    run_simulation!(engine);
    assert_eq!(dev_req_sink.num_sunk(), 0);

    // All accesses are to the same address, so only the first should be passed
    // through
    assert_eq!(mem_req_sink.num_sunk(), 1);
    assert_eq!(cache.payload_bytes_read(), num_accesses * ACCESS_SIZE_BYTES);
    assert_eq!(cache.payload_bytes_written(), 0);
}

/// Test the basics of the cache by driving/handling all of the ports manually
mod full_cache_harness {
    use super::*;

    build_model_harness! {
        harness CacheHarness<T> {
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
    fn cache() {
        let mut engine = start_test(file!());
        let cache = create_cache(&mut engine);
        let mut harness = CacheHarness::<MemoryAccess>::new(engine, cache.clone());
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
        let response = read.to_response(&TestMemory {}).unwrap();

        harness.run_steps(&[
            step_send_dev_rx(read.clone()),
            step_expect_mem_tx(
                MemoryTxn::read_req(dst_addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES),
            ),
            step_delay(memory_delay_ticks),
            step_send_mem_rx(response.clone()),
            step_expect_dev_tx(
                MemoryTxn::read_rsp(dst_addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES),
            ),
            // Make a second device request to the same address - expecting response without
            // need to go to memory
            step_parallel(HashMap::from([
                (Port::DevRx, vec![action_send_dev_rx(read)]),
                (
                    Port::DevTx,
                    vec![action_expect_dev_tx(
                        MemoryTxn::read_rsp(dst_addr)
                            .with_src_addr(SRC_ADDR)
                            .with_bytes(ACCESS_SIZE_BYTES),
                    )],
                ),
                (
                    Port::MemTx,
                    vec![action_expect_no_traffic((DELAY_TICKS * 2) as u64)],
                ),
            ])),
        ]);

        assert_eq!(cache.payload_bytes_read(), 2 * ACCESS_SIZE_BYTES);
        assert_eq!(cache.payload_bytes_written(), 0);
        assert_eq!(cache.num_misses(), 1);
        assert_eq!(cache.num_hits(), 1);
    }
}

/// Test a cache with a cache connected directly to a memory model
mod dev_cache_harness {
    use super::*;

    build_model_harness! {
        harness CacheDevHarness<T> {
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
            steps.push(step_send_dev_rx(read.clone()));
            steps.push(step_expect_dev_tx(
                MemoryTxn::read_rsp(dst_addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES),
            ));
        }

        harness.run_steps(&steps);

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
                steps.push(step_send_dev_rx(read));
                steps.push(step_expect_dev_tx(
                    MemoryTxn::read_rsp(dst_addr)
                        .with_src_addr(SRC_ADDR)
                        .with_bytes(ACCESS_SIZE_BYTES),
                ));
            }
        }

        harness.run_steps(&steps);

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
                steps.push(step_send_dev_rx(read));
                steps.push(step_expect_dev_tx(
                    MemoryTxn::read_rsp(dst_addr)
                        .with_src_addr(SRC_ADDR)
                        .with_bytes(ACCESS_SIZE_BYTES),
                ));
            }
        }

        harness.run_steps(&steps);

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
            steps.push(step_send_dev_rx(read.clone()));
            steps.push(step_expect_dev_tx(
                MemoryTxn::read_rsp(dst_addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES),
            ));
        }

        steps.push(step_send_dev_rx(write));

        for _ in 0..num_rereads {
            steps.push(step_send_dev_rx(read.clone()));
            steps.push(step_expect_dev_tx(
                MemoryTxn::read_rsp(dst_addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES),
            ));
        }

        harness.run_steps(&steps);

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
