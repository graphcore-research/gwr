// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cmp::max;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::SimObject;
use gwr_models::memory::traits::AccessMemory;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{
    create_default_memory_map, create_read, create_write, create_write_np,
};

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

        harness.run_steps([par!([
            seq!(vec![send_rx!(request); num_accesses]),
            seq!(vec![
                expect_tx!(
                    MemoryTxn::read_rsp(DST_ADDR)
                        .with_bytes(ACCESS_SIZE_BYTES)
                        .with_total_bytes(ACCESS_SIZE_BYTES + OVERHEAD_SIZE_BYTES)
                );
                num_accesses
            ]),
        ])]);

        assert_eq!(memory.bytes_read(), num_accesses * ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_written(), 0);

        let last_bw_limit_event = CYCLES_PER_ACCESS * num_accesses as u64;
        let last_packet_ack = CYCLES_PER_ACCESS * ((num_accesses - 1) as u64) + DELAY_TICKS as u64;
        let last_event_time = max(last_bw_limit_event, last_packet_ack);

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
            expect_no_traffic!(&[Port::Tx], CYCLES_PER_ACCESS),
        ]);

        assert_eq!(memory.bytes_written(), num_accesses * ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_read(), 0);

        // Simulation will only complete once the Memory has finished handling all the
        // delay imposed by the data it is carrying
        let last_bw_limit_event = CYCLES_PER_ACCESS * num_accesses as u64;
        let last_event_time = last_bw_limit_event;
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

        harness.run_steps([par!([
            seq!(vec![send_rx!(request); num_accesses]),
            seq!(vec![
                expect_tx!(
                    MemoryTxn::write_np_rsp(DST_ADDR)
                        .with_bytes(ACCESS_SIZE_BYTES)
                        .with_total_bytes(OVERHEAD_SIZE_BYTES)
                );
                num_accesses
            ]),
        ])]);

        assert_eq!(memory.bytes_written(), num_accesses * ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_read(), 0);

        let last_bw_limit_event = CYCLES_PER_ACCESS * num_accesses as u64;
        let last_packet_ack = CYCLES_PER_ACCESS * ((num_accesses - 1) as u64) + DELAY_TICKS as u64;
        let last_event_time = max(last_bw_limit_event, last_packet_ack);
        assert_eq!(harness.engine.time_now_ns(), last_event_time as f64);
    }
}
