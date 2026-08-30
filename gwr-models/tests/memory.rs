// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cmp::max;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::SimObject;
use gwr_engine::types::SimError;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::{DeviceId, MemoryMap};
use gwr_models::memory::traits::AccessMemory;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{
    create_default_memory_map, create_read, create_write, create_write_np,
};

const DST_ADDR: u64 = 0x80000;
const SRC_ADDR: u64 = DST_ADDR + 0x1000;
const CAPACITY_BYTES: usize = 0x40000;
const BW_BYTES_PER_TICK: usize = 32;
const DELAY_TICKS: usize = 8;
const ACCESS_SIZE_BYTES: usize = 128;
const OVERHEAD_SIZE_BYTES: usize = 16;

const TICKS_PER_ACCESS: u64 = (ACCESS_SIZE_BYTES as u64).div_ceil(BW_BYTES_PER_TICK as u64);

fn create_memory<T>(engine: &mut Engine) -> Rc<Memory<T>>
where
    T: SimObject + AccessMemory,
{
    let config = MemoryConfig::new(DST_ADDR, CAPACITY_BYTES, BW_BYTES_PER_TICK, DELAY_TICKS);
    let clock = engine.default_clock();
    let top = engine.top();

    Memory::new_and_register(engine, &clock, top, "memory", config).unwrap()
}

mod memory_harness {
    use gwr_models::build_model_harness;
    use gwr_models::test_helpers::MemoryTxn;

    use super::*;

    build_model_harness! {
        harness MemoryHarness<T> {
            component: memory: Rc<Memory<T>>,
            rx ports: {
                Rx<T> => rx,
            },
            tx ports: {
                Tx<T> => tx,
            },
        }
    }

    fn memory_construction_error(config: MemoryConfig) -> SimError {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let top = engine.top();
        let Err(error) =
            Memory::<MemoryAccess>::new_and_register(&engine, &clock, top, "memory", config)
        else {
            panic!("Memory construction should fail");
        };
        error
    }

    #[test]
    fn memory_rejects_zero_capacity() {
        let config = MemoryConfig::new(DST_ADDR, 0, BW_BYTES_PER_TICK, DELAY_TICKS);

        let error = memory_construction_error(config);

        assert_eq!(
            error.to_string(),
            "top::memory: capacity must be greater than zero"
        );
    }

    #[test]
    fn memory_rejects_address_range_overflow() {
        let config = MemoryConfig::new(u64::MAX, 2, BW_BYTES_PER_TICK, DELAY_TICKS);

        let error = memory_construction_error(config);

        assert_eq!(
            error.to_string(),
            "top::memory: address range starting at 0xffffffffffffffff with capacity 2 bytes exceeds the physical address space"
        );
    }

    #[test]
    fn memory_rejects_zero_bandwidth() {
        let config = MemoryConfig::new(DST_ADDR, CAPACITY_BYTES, 0, DELAY_TICKS);

        let error = memory_construction_error(config);

        assert_eq!(
            error.to_string(),
            "top::memory: bandwidth must be greater than zero"
        );
    }

    #[test]
    fn memory_read() {
        let num_accesses = 100;

        let mut engine = start_test(file!());
        let memory = create_memory(&mut engine);
        let memory_map = Rc::new(create_default_memory_map());

        let request = create_read(
            engine.top(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            DST_ADDR,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );

        let mut harness = MemoryHarness::new(engine, memory.clone());

        harness.run_steps([
            par!([
                seq!(vec![send_rx!(request); num_accesses]),
                seq!(vec![
                    expect_tx!(
                        MemoryTxn::read_rsp(DST_ADDR)
                            .with_bytes(ACCESS_SIZE_BYTES)
                            .with_total_bytes(ACCESS_SIZE_BYTES + OVERHEAD_SIZE_BYTES)
                    );
                    num_accesses
                ]),
            ]),
            expect_no_traffic!(&[Port::Tx], DELAY_TICKS as u64),
        ]);

        assert_eq!(memory.bytes_read(), num_accesses * ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_written(), 0);

        let last_bw_limit_event = TICKS_PER_ACCESS * num_accesses as u64;
        let last_packet_ack = TICKS_PER_ACCESS * ((num_accesses - 1) as u64) + DELAY_TICKS as u64;
        let last_event_time = max(last_bw_limit_event, last_packet_ack) + DELAY_TICKS as u64;

        assert_eq!(harness.engine.time_now_ns(), last_event_time as f64);
    }

    #[test]
    fn read_becomes_read_response() {
        let mut engine = start_test(file!());
        let memory = create_memory(&mut engine);
        let memory_map = Rc::new(create_default_memory_map());
        let dst_addr = DST_ADDR + 0x40;

        let request = create_read(
            engine.top(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            dst_addr,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );

        let mut harness = MemoryHarness::new(engine, memory);

        harness.run_steps([
            send_rx!(request),
            expect_tx!(
                MemoryTxn::read_rsp(dst_addr)
                    .with_bytes(ACCESS_SIZE_BYTES)
                    .with_total_bytes(ACCESS_SIZE_BYTES + OVERHEAD_SIZE_BYTES)
            ),
        ]);

        assert_eq!(harness.engine.time_now_ns(), DELAY_TICKS as f64);
    }

    #[test]
    fn memory_at_last_address_accepts_one_byte_access() {
        let mut engine = start_test(file!());
        let config = MemoryConfig::new(u64::MAX, 1, 1, 0);
        let clock = engine.default_clock();
        let top = engine.top();
        let memory = Memory::new_and_register(&engine, &clock, top, "memory", config).unwrap();
        let mut memory_map = MemoryMap::new();
        memory_map.insert(0, 1, DeviceId(0)).unwrap();
        memory_map.insert(u64::MAX, 1, DeviceId(1)).unwrap();
        let memory_map = Rc::new(memory_map);
        let request = create_read(engine.top(), &memory_map, 1, u64::MAX, 0, 0);
        let mut harness = MemoryHarness::new(engine, memory);

        harness.run_steps([
            send_rx!(request),
            expect_tx!(
                MemoryTxn::read_rsp(u64::MAX)
                    .with_bytes(1)
                    .with_total_bytes(1)
            ),
        ]);
    }

    #[test]
    fn memory_write() {
        let num_accesses = 100;

        let mut engine = start_test(file!());
        let memory = create_memory(&mut engine);
        let memory_map = Rc::new(create_default_memory_map());

        let request = create_write(
            engine.top(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            DST_ADDR,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );

        let mut harness = MemoryHarness::new(engine, memory.clone());

        harness.run_steps([
            seq!(vec![send_rx!(request); num_accesses]),
            expect_no_traffic!(&[Port::Tx], TICKS_PER_ACCESS),
            expect_no_traffic!(&[Port::Tx], DELAY_TICKS as u64),
        ]);

        assert_eq!(memory.bytes_written(), num_accesses * ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_read(), 0);

        // Simulation will only complete once the Memory has finished handling
        // all the delay imposed by the data it is carrying
        let last_bw_limit_event = TICKS_PER_ACCESS * num_accesses as u64;
        let last_event_time = last_bw_limit_event + DELAY_TICKS as u64;
        assert_eq!(harness.engine.time_now_ns(), last_event_time as f64);
    }

    #[test]
    fn memory_write_np() {
        let num_accesses = 100;

        let mut engine = start_test(file!());
        let memory = create_memory(&mut engine);
        let memory_map = Rc::new(create_default_memory_map());

        let request = create_write_np(
            engine.top(),
            &memory_map,
            ACCESS_SIZE_BYTES,
            DST_ADDR,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );

        let mut harness = MemoryHarness::new(engine, memory.clone());

        harness.run_steps([
            par!([
                seq!(vec![send_rx!(request); num_accesses]),
                seq!(vec![
                    expect_tx!(
                        MemoryTxn::write_np_rsp(DST_ADDR)
                            .with_bytes(ACCESS_SIZE_BYTES)
                            .with_total_bytes(OVERHEAD_SIZE_BYTES)
                    );
                    num_accesses
                ]),
            ]),
            expect_no_traffic!(&[Port::Tx], DELAY_TICKS as u64),
        ]);

        assert_eq!(memory.bytes_written(), num_accesses * ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_read(), 0);

        let last_bw_limit_event = TICKS_PER_ACCESS * num_accesses as u64;
        let last_packet_ack = TICKS_PER_ACCESS * ((num_accesses - 1) as u64) + DELAY_TICKS as u64;
        let last_event_time = max(last_bw_limit_event, last_packet_ack) + DELAY_TICKS as u64;
        assert_eq!(harness.engine.time_now_ns(), last_event_time as f64);
    }
}
