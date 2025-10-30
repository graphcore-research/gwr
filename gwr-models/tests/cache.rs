// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use futures::select;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat};
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::{Routable, SimObject};
use gwr_engine::types::AccessType;
use gwr_models::memory::cache::{Cache, CacheConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::traits::{AccessMemory, ReadMemory};
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{create_read, create_write};

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
    let memory = Memory::new_and_register(&engine, &clock, top, "memory", config).unwrap();

    connect_port!(cache, mem_tx => memory, rx).unwrap();
    connect_port!(memory, tx => cache, mem_rx).unwrap();

    memory
}

#[test]
fn cache_dev_read_goes_to_mem() {
    let num_accesses = 100;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let top = engine.top();
    let source = Source::new_and_register(&engine, top, "source", None).unwrap();
    let to_put = create_read(
        &source.entity,
        ACCESS_SIZE_BYTES,
        DST_ADDR,
        SRC_ADDR,
        OVERHEAD_SIZE_BYTES,
    );
    source.set_generator(option_box_repeat!(to_put ; num_accesses));

    let config = CacheConfig::new(
        LINE_SIZE_BYTES,
        BW_BYTES_PER_CYCLE,
        NUM_SETS,
        NUM_WAYS,
        DELAY_TICKS,
    );
    let cache = Cache::new_and_register(&engine, &clock, top, "cache", config).unwrap();
    let dev_req_sink = Sink::new_and_register(&engine, &clock, top, "dev_req_sink").unwrap();
    let mem_req_sink = Sink::new_and_register(&engine, &clock, top, "mem_req_sink").unwrap();

    connect_port!(source, tx => cache, dev_rx).unwrap();
    connect_port!(cache, dev_tx => dev_req_sink, rx).unwrap();
    connect_port!(cache, mem_tx => mem_req_sink, rx).unwrap();

    // Even though we are not driving it we need to connect it.
    let mut mem_rx_driver = OutPort::new(&top, "mem_rx_driver");
    mem_rx_driver.connect(cache.port_mem_rx()).unwrap();

    run_simulation!(engine);
    assert_eq!(dev_req_sink.num_sunk(), 0);

    // All accesses are to the same address, so only the first should be passed
    // through
    assert_eq!(mem_req_sink.num_sunk(), 1);
    assert_eq!(cache.payload_bytes_read(), num_accesses * ACCESS_SIZE_BYTES);
    assert_eq!(cache.payload_bytes_written(), 0);
}

/// Helper to build a cache and four ports to drive it.
fn build_cache_and_all_ports() -> (
    Engine,
    Rc<Cache<MemoryAccess>>,
    OutPort<MemoryAccess>,
    InPort<MemoryAccess>,
    OutPort<MemoryAccess>,
    InPort<MemoryAccess>,
) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let config = CacheConfig::new(
        LINE_SIZE_BYTES,
        BW_BYTES_PER_CYCLE,
        NUM_SETS,
        NUM_WAYS,
        DELAY_TICKS,
    );
    let top = engine.top();
    let cache = Cache::new_and_register(&engine, &clock, top, "cache", config).unwrap();

    let mut dev_rx_driver = OutPort::new(&top, "dev_rx_driver");
    dev_rx_driver.connect(cache.port_dev_rx()).unwrap();

    let dev_tx_recv = InPort::new(&engine, &clock, &top, "dev_tx_recv");
    cache.connect_port_dev_tx(dev_tx_recv.state()).unwrap();

    let mut mem_rx_driver = OutPort::new(&top, "mem_rx_driver");
    mem_rx_driver.connect(cache.port_mem_rx()).unwrap();

    let mem_tx_recv = InPort::new(&engine, &clock, &top, "mem_tx_recv");
    cache.connect_port_mem_tx(mem_tx_recv.state()).unwrap();

    (
        engine,
        cache,
        dev_rx_driver,
        dev_tx_recv,
        mem_rx_driver,
        mem_tx_recv,
    )
}

/// Test the basics of the cache by driving/handling all of the ports manually
#[test]
fn cache() {
    let (mut engine, cache, dev_rx_driver, dev_tx_recv, mem_rx_driver, mem_tx_recv) =
        build_cache_and_all_ports();
    let clock = engine.default_clock();
    let memory_latency_ticks = 10;

    engine.spawn(async move {
        let dst_addr = DST_ADDR + 0x40;

        // Make a device request
        let read = create_read(
            &dev_rx_driver.entity,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        dev_rx_driver.put(read)?.await;

        // Request passed on to memory
        let mem_req = mem_tx_recv.get()?.await;
        assert_eq!(mem_req.destination(), dst_addr);
        assert_eq!(mem_req.source(), SRC_ADDR);

        clock.wait_ticks(memory_latency_ticks).await;

        // Provide response from memory
        let write = mem_req.to_response(&TestMemory {});
        mem_rx_driver.put(write)?.await;

        // Response back to device
        let response_to_dev = dev_tx_recv.get()?.await;
        assert_eq!(response_to_dev.destination(), SRC_ADDR);
        assert_eq!(response_to_dev.source(), dst_addr);
        assert_eq!(response_to_dev.access_type(), AccessType::Write);

        // Make a second device request to the same address - expecting response without
        // need to go to memory
        let read = create_read(
            &dev_rx_driver.entity,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        dev_rx_driver.put(read)?.await;

        let mut mem_req = mem_tx_recv.get()?;
        let mut response_to_dev = dev_tx_recv.get()?;

        select! {
            _ = mem_req => {
                assert!(false, "No request should be made to memory");
            }
            response = response_to_dev => {
                assert_eq!(response.destination(), SRC_ADDR);
                assert_eq!(response.access_type(), AccessType::Write);
            }
        }

        Ok(())
    });

    run_simulation!(engine);

    assert_eq!(cache.payload_bytes_read(), 2 * ACCESS_SIZE_BYTES);
    assert_eq!(cache.payload_bytes_written(), 0);
    assert_eq!(cache.num_misses(), 1);
    assert_eq!(cache.num_hits(), 1);
}

/// Helper to build a cache and the device-side ports to drive it.
fn build_cache_and_dev_ports() -> (
    Engine,
    Rc<Cache<MemoryAccess>>,
    OutPort<MemoryAccess>,
    InPort<MemoryAccess>,
) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let config = CacheConfig::new(
        LINE_SIZE_BYTES,
        BW_BYTES_PER_CYCLE,
        NUM_SETS,
        NUM_WAYS,
        DELAY_TICKS,
    );
    let top = engine.top();
    let cache = Cache::new_and_register(&engine, &clock, top, "cache", config).unwrap();

    let mut dev_rx_driver = OutPort::new(&top, "dev_rx_driver");
    dev_rx_driver.connect(cache.port_dev_rx()).unwrap();

    let dev_tx_recv = InPort::new(&engine, &clock, &top, "dev_tx_recv");
    cache.connect_port_dev_tx(dev_tx_recv.state()).unwrap();

    (engine, cache, dev_rx_driver, dev_tx_recv)
}

/// Test a cache with a cache connected directly to a memory model
#[test]
fn cache_plus_mem() {
    let (mut engine, cache, dev_rx_driver, dev_tx_recv) = build_cache_and_dev_ports();
    let memory = create_and_connect_memory(&mut engine, &cache);

    let num_rereads = 10;

    engine.spawn(async move {
        let dst_addr = DST_ADDR + 0x40;

        // Make a device request
        let read = create_read(
            &dev_rx_driver.entity,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        dev_rx_driver.put(read)?.await;

        // Expect the memory to have provided a response to the cache
        let response_to_dev = dev_tx_recv.get()?.await;
        assert_eq!(response_to_dev.destination(), SRC_ADDR);
        assert_eq!(response_to_dev.access_type(), AccessType::Write);

        for _ in 0..num_rereads {
            // Make a second device request to the same address - expecting response without
            // need to go to memory
            let read = create_read(
                &dev_rx_driver.entity,
                ACCESS_SIZE_BYTES,
                dst_addr,
                SRC_ADDR,
                OVERHEAD_SIZE_BYTES,
            );
            dev_rx_driver.put(read)?.await;

            // Expect the memory to have provided a response to the cache
            let response_to_dev = dev_tx_recv.get()?.await;
            assert_eq!(response_to_dev.destination(), SRC_ADDR);
        }

        Ok(())
    });

    run_simulation!(engine);

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
    let (mut engine, cache, dev_rx_driver, dev_tx_recv) = build_cache_and_dev_ports();
    let memory = create_and_connect_memory(&mut engine, &cache);

    let num_iterations = 10;
    engine.spawn(async move {
        for _ in 0..num_iterations {
            for i in 0..NUM_WAYS {
                // Access memory in next cache way
                let dst_addr = DST_ADDR + (i * CACHE_CAPACITY_BYTES / NUM_WAYS) as u64;

                // Make a device request
                let read = create_read(
                    &dev_rx_driver.entity,
                    ACCESS_SIZE_BYTES,
                    dst_addr,
                    SRC_ADDR,
                    OVERHEAD_SIZE_BYTES,
                );
                dev_rx_driver.put(read)?.await;

                // Expect the memory to have provided a response to the cache
                let response_to_dev = dev_tx_recv.get()?.await;
                assert_eq!(response_to_dev.destination(), SRC_ADDR);
                assert_eq!(response_to_dev.access_type(), AccessType::Write);
            }
        }

        Ok(())
    });

    run_simulation!(engine);

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
    let (mut engine, cache, dev_rx_driver, dev_tx_recv) = build_cache_and_dev_ports();
    let memory = create_and_connect_memory(&mut engine, &cache);

    let num_iterations = 10;
    engine.spawn(async move {
        for _ in 0..num_iterations {
            for i in 0..NUM_WAYS + 1 {
                // Access memory in next cache way
                let dst_addr = DST_ADDR + (i * CACHE_CAPACITY_BYTES / NUM_WAYS) as u64;

                // Make a device request
                let read = create_read(
                    &dev_rx_driver.entity,
                    ACCESS_SIZE_BYTES,
                    dst_addr,
                    SRC_ADDR,
                    OVERHEAD_SIZE_BYTES,
                );
                dev_rx_driver.put(read)?.await;

                // Expect the memory to have provided a response to the cache
                let response_to_dev = dev_tx_recv.get()?.await;
                assert_eq!(response_to_dev.destination(), SRC_ADDR);
                assert_eq!(response_to_dev.access_type(), AccessType::Write);
            }
        }

        Ok(())
    });

    run_simulation!(engine);

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
    let (mut engine, cache, dev_rx_driver, dev_tx_recv) = build_cache_and_dev_ports();
    let memory = create_and_connect_memory(&mut engine, &cache);

    let num_rereads = 3;
    engine.spawn(async move {
        let dst_addr = DST_ADDR;

        // Make a couple of reads to the same address
        for _ in 0..num_rereads {
            let read = create_read(
                &dev_rx_driver.entity,
                ACCESS_SIZE_BYTES,
                dst_addr,
                SRC_ADDR,
                OVERHEAD_SIZE_BYTES,
            );
            dev_rx_driver.put(read)?.await;

            // Handle response
            let response_to_dev = dev_tx_recv.get()?.await;
            assert_eq!(response_to_dev.destination(), SRC_ADDR);
            assert_eq!(response_to_dev.access_type(), AccessType::Write);
        }

        // Write to address to cause cache flush
        let write = create_write(
            &dev_rx_driver.entity,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        dev_rx_driver.put(write)?.await;

        // Read the memory again
        for _ in 0..num_rereads {
            let read = create_read(
                &dev_rx_driver.entity,
                ACCESS_SIZE_BYTES,
                dst_addr,
                SRC_ADDR,
                OVERHEAD_SIZE_BYTES,
            );
            dev_rx_driver.put(read)?.await;

            // Handle response
            let response_to_dev = dev_tx_recv.get()?.await;
            assert_eq!(response_to_dev.destination(), SRC_ADDR);
            assert_eq!(response_to_dev.access_type(), AccessType::Write);
        }

        Ok(())
    });

    run_simulation!(engine);

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
