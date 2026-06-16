// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::round_robin::RoundRobin;
use gwr_components::delay::Delay;
use gwr_engine::test_helpers::start_test;
use gwr_models::build_model_harness;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::test_helpers::{MemoryTxn, create_default_memory_map, create_read, create_write};
use gwr_track::entity::Entity;

const DST_ADDR: u64 = 0x80000;
const SRC_ADDR: u64 = 0x90000;
const ACCESS_SIZE_BYTES: usize = 64;
const OVERHEAD_SIZE_BYTES: usize = 8;

fn test_read(creator: &Rc<Entity>, memory_map: &Rc<MemoryMap>) -> MemoryAccess {
    test_read_at(creator, memory_map, DST_ADDR)
}

fn test_read_at(creator: &Rc<Entity>, memory_map: &Rc<MemoryMap>, dst_addr: u64) -> MemoryAccess {
    create_read(
        creator,
        memory_map,
        ACCESS_SIZE_BYTES,
        dst_addr,
        SRC_ADDR,
        OVERHEAD_SIZE_BYTES,
    )
}

mod delay_harness {
    use super::*;

    build_model_harness! {
        harness DelayHarness<T> {
            component: delay: Rc<Delay<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
        }
    }

    #[test]
    fn model_harness_can_match_read_request() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
        let memory_map = Rc::new(create_default_memory_map());
        let access = test_read(engine.top(), &memory_map);

        let mut harness = DelayHarness::new(engine, delay);
        harness.run_steps([
            step_send_rx(access.clone()),
            step_expect_tx(MemoryTxn::read_req(DST_ADDR)),
        ]);
    }

    #[test]
    fn model_harness_can_match_partial_access() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
        let memory_map = Rc::new(create_default_memory_map());
        let creator = Rc::new(Entity::new(engine.top(), "creator"));
        let access = create_write(
            &creator,
            &memory_map,
            ACCESS_SIZE_BYTES,
            DST_ADDR,
            SRC_ADDR,
            OVERHEAD_SIZE_BYTES,
        );

        let mut harness = DelayHarness::new(engine, delay);
        harness.run_steps([
            step_send_rx(access),
            step_expect_tx(MemoryTxn::write_req(DST_ADDR).with_bytes(ACCESS_SIZE_BYTES)),
        ]);
    }

    #[test]
    fn model_harness_can_delay_and_expect_no_traffic() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 3);
        let memory_map = Rc::new(create_default_memory_map());
        let access = test_read(engine.top(), &memory_map);

        let mut harness = DelayHarness::new(engine, delay);
        harness.run_steps([
            step_send_rx(access),
            step_expect_no_traffic(&[Port::Tx], 2),
            step_delay(1),
            step_expect_tx(MemoryTxn::read_req(DST_ADDR)),
        ]);
    }

    #[test]
    fn model_harness_can_drive_ports_in_parallel() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 1);
        let memory_map = Rc::new(create_default_memory_map());
        let access = test_read(engine.top(), &memory_map);

        let mut harness = DelayHarness::new(engine, delay);
        harness.run_steps([step_par([
            step_send_rx(access),
            step_expect_tx(MemoryTxn::read_req(DST_ADDR)),
        ])]);
    }
}

mod arbiter_harness {
    use super::*;

    build_model_harness! {
        harness ArbiterHarness<T> {
            component: arbiter: Rc<Arbiter<T>>,
            tx ports: {
                Tx<T> => tx
            },
            rx port arrays: {
                Rx<T> => rx {
                    count: num_rx
                }
            },
        }
    }

    #[test]
    fn model_harness_supports_indexed_rx_ports() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let arbiter = Arbiter::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "arbiter",
            2,
            Box::new(RoundRobin::new()),
        );
        let memory_map = Rc::new(create_default_memory_map());
        let top = engine.top().clone();

        let addr0 = DST_ADDR;
        let addr1 = DST_ADDR + 0x1000;
        let addr2 = DST_ADDR + 0x2000;
        let addr3 = DST_ADDR + 0x3000;

        let mut harness = ArbiterHarness::new(engine, arbiter, 2);
        harness.run_steps([step_par([
            step_seq([
                step_send_rx(0, test_read_at(&top, &memory_map, addr0)),
                step_send_rx(0, test_read_at(&top, &memory_map, addr2)),
            ]),
            step_seq([
                step_send_rx(1, test_read_at(&top, &memory_map, addr1)),
                step_send_rx(1, test_read_at(&top, &memory_map, addr3)),
            ]),
            step_seq([
                step_expect_tx(MemoryTxn::read_req(addr0)),
                step_expect_tx(MemoryTxn::read_req(addr1)),
                step_expect_tx(MemoryTxn::read_req(addr2)),
                step_expect_tx(MemoryTxn::read_req(addr3)),
            ]),
        ])]);
    }
}
