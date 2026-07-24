// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::test_helpers::start_test;
use gwr_engine::types::AccessType;
use gwr_models::build_model_harness;
use gwr_models::cache::coherency_manager::{CoherenceOp, CoherencyManager, CoherencyManagerConfig};
use gwr_models::cache::traits::CoherentAccess;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::{DeviceId, MemoryMap};
use gwr_models::memory::traits::{AccessMemory, ReadMemory};
use gwr_models::test_helpers::MemoryTxn;
use gwr_track::entity::Entity;

const BASE_ADDRESS: u64 = 0x80000;
const DST_ADDR: u64 = BASE_ADDRESS + 0x100;
const SRC_ADDR: u64 = BASE_ADDRESS + 0x1000;

const LINE_SIZE_BYTES: usize = 32;
const ACCESS_SIZE_BYTES: usize = LINE_SIZE_BYTES;
const OVERHEAD_SIZE_BYTES: usize = 16;

const CACHE_A_DEVICE_ID: DeviceId = DeviceId(0);
const CACHE_B_DEVICE_ID: DeviceId = DeviceId(1);
const CACHE_C_DEVICE_ID: DeviceId = DeviceId(2);
const DIRECTORY_DEVICE_ID: DeviceId = DeviceId(3);
const BACKING_MEMORY_DEVICE_ID: DeviceId = DeviceId(4);

struct TestMemory;

impl ReadMemory for TestMemory {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn create_backing_memory_map() -> MemoryMap {
    MemoryMap::from_regions(&[(
        BASE_ADDRESS,
        u64::MAX - BASE_ADDRESS,
        BACKING_MEMORY_DEVICE_ID,
    )])
    .unwrap()
}

fn create_manager(engine: &mut Engine) -> Rc<CoherencyManager<MemoryAccess>> {
    create_manager_with_map(engine, create_backing_memory_map())
}

fn create_manager_with_map(
    engine: &mut Engine,
    backing_memory_map: MemoryMap,
) -> Rc<CoherencyManager<MemoryAccess>> {
    let clock = engine.default_clock();
    CoherencyManager::new_and_register(
        engine,
        &clock,
        engine.top(),
        "directory",
        CoherencyManagerConfig::new(LINE_SIZE_BYTES, DIRECTORY_DEVICE_ID, backing_memory_map),
    )
    .unwrap()
}

fn cache_request(
    created_by: &Rc<Entity>,
    access_type: AccessType,
    src_device: DeviceId,
    dst_addr: u64,
    coherence_op: CoherenceOp,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        access_type,
        ACCESS_SIZE_BYTES,
        dst_addr,
        SRC_ADDR,
        DIRECTORY_DEVICE_ID,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
    .with_coherence_op(Some(coherence_op))
}

fn cache_request_without_coherence_op(
    created_by: &Rc<Entity>,
    access_type: AccessType,
    src_device: DeviceId,
    dst_addr: u64,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        access_type,
        ACCESS_SIZE_BYTES,
        dst_addr,
        SRC_ADDR,
        DIRECTORY_DEVICE_ID,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
}

fn sized_cache_request(
    created_by: &Rc<Entity>,
    access_type: AccessType,
    src_device: DeviceId,
    dst_addr: u64,
    access_size_bytes: usize,
    coherence_op: Option<CoherenceOp>,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        access_type,
        access_size_bytes,
        dst_addr,
        SRC_ADDR,
        DIRECTORY_DEVICE_ID,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
    .with_coherence_op(coherence_op)
}

fn invalidate_ack(created_by: &Rc<Entity>, src_device: DeviceId, dst_addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::Control,
        ACCESS_SIZE_BYTES,
        dst_addr,
        SRC_ADDR,
        DIRECTORY_DEVICE_ID,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
    .with_coherence_op(Some(CoherenceOp::InvalidateAck))
}

fn barrier_request(created_by: &Rc<Entity>, src_device: DeviceId, dst_addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::BarrierRequest,
        ACCESS_SIZE_BYTES,
        dst_addr,
        SRC_ADDR,
        DIRECTORY_DEVICE_ID,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
}

fn memory_read_request(created_by: &Rc<Entity>, dst_addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::ReadRequest,
        ACCESS_SIZE_BYTES,
        dst_addr,
        SRC_ADDR,
        BACKING_MEMORY_DEVICE_ID,
        DIRECTORY_DEVICE_ID,
        OVERHEAD_SIZE_BYTES,
    )
}

fn memory_read_response(created_by: &Rc<Entity>, dst_addr: u64) -> MemoryAccess {
    memory_read_request(created_by, dst_addr)
        .to_response(&TestMemory)
        .unwrap()
}

fn memory_read_txn(dst_addr: u64) -> MemoryTxn {
    MemoryTxn::read_req(dst_addr)
        .with_src_device(DIRECTORY_DEVICE_ID)
        .with_dst_device(BACKING_MEMORY_DEVICE_ID)
        .with_coherence_op(None)
}

fn memory_write_txn(dst_addr: u64) -> MemoryTxn {
    MemoryTxn::write_req(dst_addr)
        .with_src_device(DIRECTORY_DEVICE_ID)
        .with_dst_device(BACKING_MEMORY_DEVICE_ID)
        .with_coherence_op(None)
}

fn grant_shared_txn(dst_device: DeviceId, dst_addr: u64) -> MemoryTxn {
    MemoryTxn::read_rsp(dst_addr)
        .with_src_device(DIRECTORY_DEVICE_ID)
        .with_dst_device(dst_device)
        .with_coherence_op(Some(CoherenceOp::GrantShared))
}

fn invalidate_txn(dst_device: DeviceId, dst_addr: u64) -> MemoryTxn {
    MemoryTxn::control(dst_addr)
        .with_src_device(DIRECTORY_DEVICE_ID)
        .with_dst_device(dst_device)
        .with_coherence_op(Some(CoherenceOp::Invalidate))
}

fn grant_exclusive_txn(dst_device: DeviceId, dst_addr: u64) -> MemoryTxn {
    MemoryTxn::control(dst_addr)
        .with_src_device(DIRECTORY_DEVICE_ID)
        .with_dst_device(dst_device)
        .with_coherence_op(Some(CoherenceOp::GrantExclusive))
}

fn barrier_response_txn(dst_device: DeviceId, dst_addr: u64) -> MemoryTxn {
    MemoryTxn::barrier_rsp(dst_addr)
        .with_src_device(DIRECTORY_DEVICE_ID)
        .with_dst_device(dst_device)
}

mod harness {
    use gwr_engine::traits::SimObject;

    use super::*;

    build_model_harness! {
        harness ManagerHarness<T> {
            component: manager: Rc<CoherencyManager<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
        }
    }

    impl<T> ManagerHarness<T>
    where
        T: SimObject + CoherentAccess,
    {
        fn read_a(&self) -> MemoryAccess {
            cache_request(
                &self.entity,
                AccessType::ReadRequest,
                CACHE_A_DEVICE_ID,
                DST_ADDR,
                CoherenceOp::SharedRead,
            )
        }

        fn read_b(&self) -> MemoryAccess {
            cache_request(
                &self.entity,
                AccessType::ReadRequest,
                CACHE_B_DEVICE_ID,
                DST_ADDR,
                CoherenceOp::SharedRead,
            )
        }

        fn plain_read_b(&self) -> MemoryAccess {
            cache_request_without_coherence_op(
                &self.entity,
                AccessType::ReadRequest,
                CACHE_B_DEVICE_ID,
                DST_ADDR,
            )
        }

        fn write_np_a(&self) -> MemoryAccess {
            cache_request(
                &self.entity,
                AccessType::WriteNonPostedRequest,
                CACHE_A_DEVICE_ID,
                DST_ADDR,
                CoherenceOp::ExclusiveWrite,
            )
        }

        fn write_np_c(&self) -> MemoryAccess {
            cache_request(
                &self.entity,
                AccessType::WriteNonPostedRequest,
                CACHE_C_DEVICE_ID,
                DST_ADDR,
                CoherenceOp::ExclusiveWrite,
            )
        }

        fn owner_write_a(&self) -> MemoryAccess {
            cache_request_without_coherence_op(
                &self.entity,
                AccessType::WriteRequest,
                CACHE_A_DEVICE_ID,
                DST_ADDR,
            )
        }

        fn plain_write_a(&self) -> MemoryAccess {
            self.owner_write_a()
        }

        fn barrier_a(&self) -> MemoryAccess {
            barrier_request(&self.entity, CACHE_A_DEVICE_ID, DST_ADDR)
        }

        fn barrier_b(&self) -> MemoryAccess {
            barrier_request(&self.entity, CACHE_B_DEVICE_ID, DST_ADDR)
        }

        fn invalidate_ack_a(&self) -> MemoryAccess {
            invalidate_ack(&self.entity, CACHE_A_DEVICE_ID, DST_ADDR)
        }

        fn invalidate_ack_b(&self) -> MemoryAccess {
            invalidate_ack(&self.entity, CACHE_B_DEVICE_ID, DST_ADDR)
        }

        fn read_response(&self) -> MemoryAccess {
            memory_read_response(&self.entity, DST_ADDR)
        }
    }

    #[test]
    fn nonposted_write_invalidates_sharer_and_completes_without_immediate_memory_write() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager.clone());

        let read_a = harness.read_a();
        let read_b = harness.read_b();
        let read_response = harness.read_response();
        let write_np_a = harness.write_np_a();

        let invalidate_ack_b = invalidate_ack(&harness.entity, CACHE_B_DEVICE_ID, DST_ADDR);
        let reread_b = harness.read_b();
        let invalidate_ack_a = invalidate_ack(&harness.entity, CACHE_A_DEVICE_ID, DST_ADDR);
        let reread_b_response = harness.read_response();

        let memory_read_txn = memory_read_txn(DST_ADDR);
        let grant_shared_a_txn = grant_shared_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let grant_shared_b_txn = grant_shared_txn(CACHE_B_DEVICE_ID, DST_ADDR);
        let invalidate_b_txn = invalidate_txn(CACHE_B_DEVICE_ID, DST_ADDR);
        let grant_exclusive_a_txn = grant_exclusive_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let invalidate_a_txn = invalidate_txn(CACHE_A_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(read_a),
            expect_tx!(memory_read_txn.clone()),
            send_rx!(read_response.clone()),
            expect_tx!(grant_shared_a_txn),
            send_rx!(read_b),
            expect_tx!(memory_read_txn.clone()),
            send_rx!(read_response),
            expect_tx!(grant_shared_b_txn.clone()),
            send_rx!(write_np_a),
            expect_tx!(invalidate_b_txn),
            send_rx!(invalidate_ack_b),
            expect_tx!(grant_exclusive_a_txn),
            send_rx!(reread_b),
            expect_tx!(invalidate_a_txn),
            send_rx!(invalidate_ack_a),
            expect_tx!(memory_read_txn),
            send_rx!(reread_b_response),
            expect_tx!(grant_shared_b_txn),
        ]);

        manager.dump_stats(0.0);
        assert_eq!(manager.op_received_count(CoherenceOp::SharedRead), 3);
        assert_eq!(manager.op_received_count(CoherenceOp::ExclusiveWrite), 1);
        assert_eq!(manager.op_received_count(CoherenceOp::Invalidate), 0);
        assert_eq!(manager.op_received_count(CoherenceOp::InvalidateAck), 2);
        assert_eq!(manager.op_received_count(CoherenceOp::GrantShared), 0);
        assert_eq!(manager.op_received_count(CoherenceOp::GrantExclusive), 0);
        assert_eq!(manager.op_sent_count(CoherenceOp::SharedRead), 0);
        assert_eq!(manager.op_sent_count(CoherenceOp::ExclusiveWrite), 0);
        assert_eq!(manager.op_sent_count(CoherenceOp::Invalidate), 2);
        assert_eq!(manager.op_sent_count(CoherenceOp::InvalidateAck), 0);
        assert_eq!(manager.op_sent_count(CoherenceOp::GrantShared), 3);
        assert_eq!(manager.op_sent_count(CoherenceOp::GrantExclusive), 1);
    }

    #[test]
    fn barrier_waits_for_outstanding_manager_work_and_blocks_later_requests() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager.clone());

        let read_a = harness.read_a();
        let barrier_a = barrier_request(&harness.entity, CACHE_A_DEVICE_ID, DST_ADDR);
        let read_b = harness.read_b();
        let read_a_response = harness.read_response();

        let memory_read_txn = memory_read_txn(DST_ADDR);
        let grant_shared_a_txn = grant_shared_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let barrier_a_response_txn = barrier_response_txn(CACHE_A_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(read_a),
            expect_tx!(memory_read_txn.clone()),
            send_rx!(barrier_a),
            send_rx!(read_b),
            send_rx!(read_a_response),
            expect_tx!(grant_shared_a_txn),
            expect_tx!(barrier_a_response_txn),
            expect_tx!(memory_read_txn),
        ]);

        assert_eq!(manager.op_received_count(CoherenceOp::SharedRead), 2);
        assert_eq!(manager.total_received_count(), 2);
        assert_eq!(manager.op_sent_count(CoherenceOp::GrantShared), 1);
        assert_eq!(manager.total_sent_count(), 1);
    }

    #[test]
    fn plain_write_from_non_owner_is_upgraded_to_exclusive_request() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let plain_write_a = harness.plain_write_a();

        let grant_exclusive_a_txn = grant_exclusive_txn(CACHE_A_DEVICE_ID, DST_ADDR);

        harness.run_steps([send_rx!(plain_write_a), expect_tx!(grant_exclusive_a_txn)]);
    }

    #[test]
    fn owner_plain_write_is_forwarded_to_backing_memory() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let write_np_a = harness.write_np_a();
        let owner_write_a = harness.owner_write_a();

        let grant_exclusive_a_txn = grant_exclusive_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let memory_write_txn = memory_write_txn(DST_ADDR);

        harness.run_steps([
            send_rx!(write_np_a),
            expect_tx!(grant_exclusive_a_txn),
            send_rx!(owner_write_a),
            expect_tx!(memory_write_txn),
        ]);
    }

    #[test]
    fn shared_read_invalidates_owner_before_reading_memory() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let write_np_a = harness.write_np_a();
        let read_b = harness.read_b();
        let invalidate_ack_a = harness.invalidate_ack_a();
        let read_b_response = harness.read_response();

        let grant_exclusive_a_txn = grant_exclusive_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let invalidate_a_txn = invalidate_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let memory_read_txn = memory_read_txn(DST_ADDR);
        let grant_shared_b_txn = grant_shared_txn(CACHE_B_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(write_np_a),
            expect_tx!(grant_exclusive_a_txn),
            send_rx!(read_b),
            expect_tx!(invalidate_a_txn),
            send_rx!(invalidate_ack_a),
            expect_tx!(memory_read_txn),
            send_rx!(read_b_response),
            expect_tx!(grant_shared_b_txn),
        ]);
    }

    #[test]
    fn plain_read_invalidates_owner_before_reading_memory() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let write_np_a = harness.write_np_a();
        let plain_read_b = harness.plain_read_b();
        let invalidate_ack_a = harness.invalidate_ack_a();
        let read_b_response = harness.read_response();

        let grant_exclusive_a_txn = grant_exclusive_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let invalidate_a_txn = invalidate_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let memory_read_txn = memory_read_txn(DST_ADDR);
        let grant_shared_b_txn = grant_shared_txn(CACHE_B_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(write_np_a),
            expect_tx!(grant_exclusive_a_txn),
            send_rx!(plain_read_b),
            expect_tx!(invalidate_a_txn),
            send_rx!(invalidate_ack_a),
            expect_tx!(memory_read_txn),
            send_rx!(read_b_response),
            expect_tx!(grant_shared_b_txn),
        ]);
    }

    #[test]
    fn exclusive_write_invalidates_owner_before_granting_new_owner() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let write_np_a = harness.write_np_a();
        let write_np_c = harness.write_np_c();
        let invalidate_ack_a = harness.invalidate_ack_a();

        let grant_exclusive_a_txn = grant_exclusive_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let invalidate_a_txn = invalidate_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let grant_exclusive_c_txn = grant_exclusive_txn(CACHE_C_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(write_np_a),
            expect_tx!(grant_exclusive_a_txn),
            send_rx!(write_np_c),
            expect_tx!(invalidate_a_txn),
            send_rx!(invalidate_ack_a),
            expect_tx!(grant_exclusive_c_txn),
        ]);
    }

    #[test]
    fn owner_writeback_is_forwarded_while_invalidation_waits_for_ack() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let write_np_a = harness.write_np_a();
        let read_b = harness.read_b();
        let owner_write_a = harness.owner_write_a();
        let invalidate_ack_a = harness.invalidate_ack_a();

        let grant_exclusive_a_txn = grant_exclusive_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let invalidate_a_txn = invalidate_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let memory_write_txn = memory_write_txn(DST_ADDR);
        let memory_read_txn = memory_read_txn(DST_ADDR);

        harness.run_steps([
            send_rx!(write_np_a),
            expect_tx!(grant_exclusive_a_txn),
            send_rx!(read_b),
            expect_tx!(invalidate_a_txn),
            send_rx!(owner_write_a),
            expect_tx!(memory_write_txn),
            send_rx!(invalidate_ack_a),
            expect_tx!(memory_read_txn),
        ]);
    }

    #[test]
    fn exclusive_write_waits_for_all_sharer_invalidations_before_granting() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let read_a = harness.read_a();
        let read_a_response = harness.read_response();
        let read_b = harness.read_b();
        let read_b_response = harness.read_response();
        let write_np_c = harness.write_np_c();
        let invalidate_ack_a = harness.invalidate_ack_a();
        let invalidate_ack_b = harness.invalidate_ack_b();

        let memory_read_txn = memory_read_txn(DST_ADDR);
        let grant_shared_a_txn = grant_shared_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let grant_shared_b_txn = grant_shared_txn(CACHE_B_DEVICE_ID, DST_ADDR);
        let invalidate_a_txn = invalidate_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let invalidate_b_txn = invalidate_txn(CACHE_B_DEVICE_ID, DST_ADDR);
        let grant_exclusive_c_txn = grant_exclusive_txn(CACHE_C_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(read_a),
            expect_tx!(memory_read_txn.clone()),
            send_rx!(read_a_response),
            expect_tx!(grant_shared_a_txn),
            send_rx!(read_b),
            expect_tx!(memory_read_txn),
            send_rx!(read_b_response),
            expect_tx!(grant_shared_b_txn),
            send_rx!(write_np_c),
            expect_tx!(invalidate_a_txn),
            expect_tx!(invalidate_b_txn),
            send_rx!(invalidate_ack_a),
            expect_no_traffic!(&[Port::Tx], 2),
            send_rx!(invalidate_ack_b),
            expect_tx!(grant_exclusive_c_txn),
        ]);
    }

    #[test]
    fn queued_same_line_read_replays_after_outstanding_read_response() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let read_a = harness.read_a();
        let read_b = harness.read_b();
        let read_a_response = harness.read_response();
        let read_b_response = harness.read_response();

        let memory_read_txn = memory_read_txn(DST_ADDR);
        let grant_shared_a_txn = grant_shared_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let grant_shared_b_txn = grant_shared_txn(CACHE_B_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(read_a),
            expect_tx!(memory_read_txn.clone()),
            send_rx!(read_b),
            expect_no_traffic!(&[Port::Tx], 2),
            send_rx!(read_a_response),
            expect_tx!(grant_shared_a_txn),
            expect_tx!(memory_read_txn),
            send_rx!(read_b_response),
            expect_tx!(grant_shared_b_txn),
        ]);
    }

    #[test]
    fn queued_same_line_plain_write_replays_after_outstanding_read_response() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let read_a = harness.read_a();
        let plain_write_a = harness.plain_write_a();
        let read_a_response = harness.read_response();

        let memory_read_txn = memory_read_txn(DST_ADDR);
        let grant_shared_a_txn = grant_shared_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let grant_exclusive_a_txn = grant_exclusive_txn(CACHE_A_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(read_a),
            expect_tx!(memory_read_txn),
            send_rx!(plain_write_a),
            expect_no_traffic!(&[Port::Tx], 2),
            send_rx!(read_a_response),
            expect_tx!(grant_shared_a_txn),
            expect_tx!(grant_exclusive_a_txn),
        ]);
    }

    #[test]
    fn second_barrier_waits_behind_active_barrier() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let read_a = harness.read_a();
        let barrier_a = harness.barrier_a();
        let barrier_b = harness.barrier_b();
        let read_a_response = harness.read_response();

        let memory_read_txn = memory_read_txn(DST_ADDR);
        let grant_shared_a_txn = grant_shared_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let barrier_a_response_txn = barrier_response_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let barrier_b_response_txn = barrier_response_txn(CACHE_B_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(read_a),
            expect_tx!(memory_read_txn),
            send_rx!(barrier_a),
            send_rx!(barrier_b),
            send_rx!(read_a_response),
            expect_tx!(grant_shared_a_txn),
            expect_tx!(barrier_a_response_txn),
            expect_tx!(barrier_b_response_txn),
        ]);
    }

    #[test]
    fn write_blocked_by_barrier_replays_after_barrier_completion() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let read_a = harness.read_a();
        let barrier_a = harness.barrier_a();
        let write_np_a = harness.write_np_a();
        let read_a_response = harness.read_response();

        let memory_read_txn = memory_read_txn(DST_ADDR);
        let grant_shared_a_txn = grant_shared_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let barrier_a_response_txn = barrier_response_txn(CACHE_A_DEVICE_ID, DST_ADDR);
        let grant_exclusive_a_txn = grant_exclusive_txn(CACHE_A_DEVICE_ID, DST_ADDR);

        harness.run_steps([
            send_rx!(read_a),
            expect_tx!(memory_read_txn),
            send_rx!(barrier_a),
            send_rx!(write_np_a),
            send_rx!(read_a_response),
            expect_tx!(grant_shared_a_txn),
            expect_tx!(barrier_a_response_txn),
            expect_tx!(grant_exclusive_a_txn),
        ]);
    }

    #[test]
    #[should_panic(expected = "No backing memory for address 0x7fff0")]
    fn read_request_errors_when_start_address_has_no_backing_memory() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let unmapped_read = sized_cache_request(
            &harness.entity,
            AccessType::ReadRequest,
            CACHE_A_DEVICE_ID,
            BASE_ADDRESS - 0x10,
            ACCESS_SIZE_BYTES,
            Some(CoherenceOp::SharedRead),
        );

        harness.run_steps([send_rx!(unmapped_read)]);
    }

    #[test]
    #[should_panic(expected = "No backing memory for address 0x8001f")]
    fn read_request_errors_when_end_address_has_no_backing_memory() {
        let mut engine = start_test(file!());
        let backing_memory_map =
            MemoryMap::from_regions(&[(BASE_ADDRESS, 0x10, BACKING_MEMORY_DEVICE_ID)]).unwrap();
        let manager = create_manager_with_map(&mut engine, backing_memory_map);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let read_spanning_beyond_map = sized_cache_request(
            &harness.entity,
            AccessType::ReadRequest,
            CACHE_A_DEVICE_ID,
            BASE_ADDRESS,
            ACCESS_SIZE_BYTES,
            Some(CoherenceOp::SharedRead),
        );

        harness.run_steps([send_rx!(read_spanning_beyond_map)]);
    }

    #[test]
    #[should_panic(expected = "Coherence access [0x80010,0x8002f] spans multiple backing memories")]
    fn read_request_errors_when_access_spans_multiple_backing_memories() {
        let mut engine = start_test(file!());
        let backing_memory_map = MemoryMap::from_regions(&[
            (BASE_ADDRESS, 0x20, BACKING_MEMORY_DEVICE_ID),
            (BASE_ADDRESS + 0x20, 0x20, DeviceId(5)),
        ])
        .unwrap();
        let manager = create_manager_with_map(&mut engine, backing_memory_map);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let spanning_read = sized_cache_request(
            &harness.entity,
            AccessType::ReadRequest,
            CACHE_A_DEVICE_ID,
            BASE_ADDRESS + 0x10,
            ACCESS_SIZE_BYTES,
            Some(CoherenceOp::SharedRead),
        );

        harness.run_steps([send_rx!(spanning_read)]);
    }

    #[test]
    #[should_panic(
        expected = "unsupported request with AccessType ReadRequest and coherence op Some(GrantShared)"
    )]
    fn unsupported_cache_request_errors() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let unsupported_read = cache_request(
            &harness.entity,
            AccessType::ReadRequest,
            CACHE_A_DEVICE_ID,
            DST_ADDR,
            CoherenceOp::GrantShared,
        );

        harness.run_steps([send_rx!(unsupported_read)]);
    }

    #[test]
    #[should_panic(expected = "Unexpected invalidate ack for line")]
    fn unexpected_invalidate_ack_errors() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let invalidate_ack_a = harness.invalidate_ack_a();

        harness.run_steps([send_rx!(invalidate_ack_a)]);
    }

    #[test]
    #[should_panic(expected = "Unexpected memory response for line")]
    fn unexpected_memory_response_errors() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let read_response = harness.read_response();

        harness.run_steps([send_rx!(read_response)]);
    }

    #[test]
    #[should_panic(
        expected = "unsupported request with AccessType ReadResponse and coherence op None"
    )]
    fn response_addressed_to_cache_is_not_treated_as_memory_response() {
        let mut engine = start_test(file!());
        let manager = create_manager(&mut engine);
        let mut harness = ManagerHarness::<MemoryAccess>::new(engine, manager);

        let cache_bound_response = MemoryAccess::new(
            &harness.entity,
            AccessType::ReadResponse,
            ACCESS_SIZE_BYTES,
            DST_ADDR,
            SRC_ADDR,
            CACHE_A_DEVICE_ID,
            BACKING_MEMORY_DEVICE_ID,
            OVERHEAD_SIZE_BYTES,
        );

        harness.run_steps([send_rx!(cache_bound_response)]);
    }
}
