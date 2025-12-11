// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cmp::max;
use std::rc::Rc;

use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat};
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::AccessType;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::memory::traits::AccessMemory;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{
    create_default_memory_map, create_read, create_write, create_write_np,
};
use gwr_track::entity::{Entity, GetEntity};

const DST_ADDR: u64 = 0x80000;
const SRC_ADDR: u64 = DST_ADDR + 0x1000;
const CAPACITY_BYTES: usize = 0x40000;
const BW_BYTES_PER_CYCLE: usize = 32;
const DELAY_TICKS: usize = 8;
const ACCESS_SIZE_BYTES: usize = 128;
const OVERHEAD_SIZE_BYTES: usize = 16;

const CYCLES_PER_ACCESS: u64 = (ACCESS_SIZE_BYTES as u64).div_ceil(BW_BYTES_PER_CYCLE as u64);

fn create_memory<T>(engine: &mut Engine) -> Rc<Memory<T>>
where
    T: SimObject + AccessMemory,
{
    let config = MemoryConfig::new(DST_ADDR, CAPACITY_BYTES, BW_BYTES_PER_CYCLE, DELAY_TICKS);
    let clock = engine.default_clock();
    let top = engine.top();
    let memory = Memory::new_and_register(&engine, &clock, top, "memory", config).unwrap();
    memory
}

fn setup_system(
    num_accesses: usize,
    create_fn: fn(&Rc<Entity>, &Rc<MemoryMap>, usize, u64, u64, usize) -> MemoryAccess,
) -> (Engine, Rc<Sink<MemoryAccess>>, Rc<Memory<MemoryAccess>>) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let memory = create_memory(&mut engine);
    let memory_map = Rc::new(create_default_memory_map());
    let top = engine.top();

    let source = Source::new_and_register(&engine, top, "source", None).unwrap();
    let to_put = create_fn(
        source.entity(),
        &memory_map,
        ACCESS_SIZE_BYTES,
        DST_ADDR,
        SRC_ADDR,
        OVERHEAD_SIZE_BYTES,
    );
    source.set_generator(option_box_repeat!(to_put ; num_accesses));

    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    connect_port!(source, tx => memory, rx).unwrap();
    connect_port!(memory, tx => sink, rx).unwrap();

    (engine, sink, memory)
}

#[test]
fn memory_read() {
    let num_accesses = 100;
    let (mut engine, sink, memory) = setup_system(num_accesses, create_read);

    run_simulation!(engine);
    assert_eq!(sink.num_sunk(), num_accesses);
    assert_eq!(memory.bytes_read(), (num_accesses * ACCESS_SIZE_BYTES));
    assert_eq!(memory.bytes_written(), 0);

    let last_bw_limit_event = CYCLES_PER_ACCESS * num_accesses as u64;
    let last_packet_ack = CYCLES_PER_ACCESS * ((num_accesses - 1) as u64) + DELAY_TICKS as u64;
    let last_event_time = max(last_bw_limit_event, last_packet_ack);
    assert_eq!(engine.time_now_ns(), last_event_time as f64);
}

#[test]
fn memory_write() {
    let num_accesses = 100;
    let (mut engine, sink, memory) = setup_system(num_accesses, create_write);

    run_simulation!(engine);
    assert_eq!(sink.num_sunk(), 0);
    assert_eq!(memory.bytes_written(), num_accesses * ACCESS_SIZE_BYTES);
    assert_eq!(memory.bytes_read(), 0);

    // Simulation will only complete once the Memory has finished handling all the
    // delay imposed by the data it is carrying
    let last_bw_limit_event = CYCLES_PER_ACCESS * num_accesses as u64;
    let last_event_time = last_bw_limit_event;
    assert_eq!(engine.time_now_ns(), last_event_time as f64);
}

#[test]
fn memory_write_np() {
    let num_accesses = 100;
    let (mut engine, sink, memory) = setup_system(num_accesses, create_write_np);

    run_simulation!(engine);

    assert_eq!(sink.num_sunk(), num_accesses);
    assert_eq!(memory.bytes_written(), num_accesses * ACCESS_SIZE_BYTES);
    assert_eq!(memory.bytes_read(), 0);

    let last_bw_limit_event = CYCLES_PER_ACCESS * num_accesses as u64;
    let last_packet_ack = CYCLES_PER_ACCESS * ((num_accesses - 1) as u64) + DELAY_TICKS as u64;
    let last_event_time = max(last_bw_limit_event, last_packet_ack);
    assert_eq!(engine.time_now_ns(), last_event_time as f64);
}

#[test]
fn read_becomes_read_response() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let memory = create_memory(&mut engine);
    let memory_map = Rc::new(create_default_memory_map());
    let top = engine.top();

    let mut mem_rx_driver = OutPort::new(&top, "mem_rx_driver");
    mem_rx_driver.connect(memory.port_rx()).unwrap();

    let mem_tx_recv = InPort::new(&engine, &clock, &top, "mem_tx_recv");
    memory.connect_port_tx(mem_tx_recv.state()).unwrap();

    engine.spawn(async move {
        let dst_addr = DST_ADDR + 0x40;

        // Make a device request
        let request = create_read(
            mem_rx_driver.entity(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );
        mem_rx_driver.put(request)?.await;

        let response = mem_tx_recv.get()?.await;

        // The memory should reply to a Read with a ReadResponse
        assert_eq!(response.access_type(), AccessType::ReadResponse);
        assert_eq!(response.access_size_bytes(), ACCESS_SIZE_BYTES);
        assert_eq!(
            response.total_bytes(),
            ACCESS_SIZE_BYTES + OVERHEAD_SIZE_BYTES
        );

        Ok(())
    });

    run_simulation!(engine);

    assert_eq!(engine.time_now_ns(), DELAY_TICKS as f64);
}
