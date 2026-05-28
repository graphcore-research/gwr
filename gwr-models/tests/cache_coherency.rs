// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_components::connect_port;
use gwr_engine::engine::Engine;
use gwr_engine::port::PortStateResult;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::{Routable, SimObject};
use gwr_engine::types::{AccessType, SimResult};
use gwr_models::build_model_harness;
use gwr_models::cache::coherency_manager::{CoherenceOp, CoherencyManager, CoherencyManagerConfig};
use gwr_models::cache::traits::CoherentAccess;
use gwr_models::cache::{Cache, CacheConfig};
use gwr_models::fabric::functional::FunctionalFabric;
use gwr_models::fabric::{Fabric, FabricConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::{DeviceId, MemoryMap};
use gwr_models::memory::traits::AccessMemory;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::MemoryTxn;
use gwr_track::entity::Entity;

const BASE_ADDRESS: u64 = 0x80000;
const DST_ADDR: u64 = BASE_ADDRESS;
const SRC_ADDR: u64 = BASE_ADDRESS + 0x1000;

const BW_BYTES_PER_CYCLE: usize = 8;
const LINE_SIZE_BYTES: usize = 32;
const NUM_SETS: usize = 1024;
const NUM_WAYS: usize = 4;

const ACCESS_SIZE_BYTES: usize = LINE_SIZE_BYTES;
const OVERHEAD_SIZE_BYTES: usize = 16;
const CACHE_CAPACITY_BYTES: usize = NUM_SETS * NUM_WAYS * LINE_SIZE_BYTES;
const DELAY_TICKS: usize = 20;
const CACHE_A_DEVICE_ID: DeviceId = DeviceId(0);
const CACHE_B_DEVICE_ID: DeviceId = DeviceId(1);
const DIRECTORY_DEVICE_ID: DeviceId = DeviceId(2);
const BACKING_MEMORY_DEVICE_ID: DeviceId = DeviceId(3);
const CPU_A_DEVICE_ID: DeviceId = DeviceId(10);
const CPU_B_DEVICE_ID: DeviceId = DeviceId(11);
const NUM_DEV_PORTS: usize = 2;

fn create_cache_config(device_id: DeviceId) -> CacheConfig {
    let memory_map =
        Rc::new(MemoryMap::from_regions(&[(0, u64::MAX, BACKING_MEMORY_DEVICE_ID)]).unwrap());
    CacheConfig::new(
        device_id,
        LINE_SIZE_BYTES,
        BW_BYTES_PER_CYCLE,
        NUM_SETS,
        NUM_WAYS,
        DELAY_TICKS,
        &memory_map,
    )
}

fn create_coherent_cache(
    engine: &mut Engine,
    coherency_manager_memory_map: MemoryMap,
) -> Rc<Cache<MemoryAccess>> {
    let clock = engine.default_clock();
    Cache::new_and_register(
        engine,
        &clock,
        engine.top(),
        "cache",
        create_cache_config(CACHE_A_DEVICE_ID)
            .with_coherency_manager_memory_map(&Rc::new(coherency_manager_memory_map)),
    )
    .unwrap()
}

fn read_for_device(created_by: &Rc<Entity>, src_device: DeviceId, dst_addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::ReadRequest,
        ACCESS_SIZE_BYTES,
        dst_addr,
        SRC_ADDR,
        BACKING_MEMORY_DEVICE_ID,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
}

fn write_for_device(created_by: &Rc<Entity>, src_device: DeviceId, dst_addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::WriteRequest,
        ACCESS_SIZE_BYTES,
        dst_addr,
        SRC_ADDR,
        BACKING_MEMORY_DEVICE_ID,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
}

fn write_np_for_device(
    created_by: &Rc<Entity>,
    src_device: DeviceId,
    dst_addr: u64,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::WriteNonPostedRequest,
        ACCESS_SIZE_BYTES,
        dst_addr,
        SRC_ADDR,
        BACKING_MEMORY_DEVICE_ID,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
}

fn barrier(created_by: &Rc<Entity>, dst_addr: u64, src_device: DeviceId) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::BarrierRequest,
        0,
        dst_addr,
        0,
        DIRECTORY_DEVICE_ID,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
}

fn invalidate(
    created_by: &Rc<Entity>,
    addr: u64,
    dst_device: DeviceId,
    src_device: DeviceId,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::Control,
        ACCESS_SIZE_BYTES,
        addr,
        SRC_ADDR,
        dst_device,
        src_device,
        OVERHEAD_SIZE_BYTES,
    )
    .with_coherence_op(Some(CoherenceOp::Invalidate))
}

mod cache_harness {
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
    fn coherent_cache_routes_requests_to_manager_by_address() {
        const BACKING_ADDRESS: u64 = BASE_ADDRESS + LINE_SIZE_BYTES as u64;
        let coherency_manager_memory_map = MemoryMap::from_regions(&[
            (BASE_ADDRESS, LINE_SIZE_BYTES as u64, DIRECTORY_DEVICE_ID),
            (
                BACKING_ADDRESS,
                LINE_SIZE_BYTES as u64,
                BACKING_MEMORY_DEVICE_ID,
            ),
            (SRC_ADDR, LINE_SIZE_BYTES as u64, CACHE_A_DEVICE_ID),
        ])
        .unwrap();

        let mut engine = start_test(file!());
        let cache = create_coherent_cache(&mut engine, coherency_manager_memory_map);
        let mut harness = CacheHarness::<MemoryAccess>::new(engine, cache);

        let base_read = read_for_device(&harness.entity, CPU_A_DEVICE_ID, BASE_ADDRESS);
        let backing_read = read_for_device(&harness.entity, CPU_A_DEVICE_ID, BACKING_ADDRESS);

        let base_read_txn = MemoryTxn::read_req(BASE_ADDRESS)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::SharedRead))
            .with_src_addr(SRC_ADDR)
            .with_bytes(ACCESS_SIZE_BYTES);
        let backing_read_txn = MemoryTxn::read_req(BACKING_ADDRESS)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(BACKING_MEMORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::SharedRead))
            .with_src_addr(SRC_ADDR)
            .with_bytes(ACCESS_SIZE_BYTES);

        harness.run_steps([par!([
            seq!([send_dev_rx!(base_read), send_dev_rx!(backing_read)]),
            seq!([
                expect_mem_tx!(base_read_txn),
                expect_mem_tx!(backing_read_txn),
            ]),
        ])]);
    }

    #[test]
    fn coherent_cache_invalidate_forces_reread_miss() {
        let coherency_manager_memory_map =
            MemoryMap::from_regions(&[(0u64, u64::MAX, DIRECTORY_DEVICE_ID)]).unwrap();
        let mut engine = start_test(file!());
        let cache = create_coherent_cache(&mut engine, coherency_manager_memory_map);
        let mut harness = CacheHarness::<MemoryAccess>::new(engine, cache);
        let addr = DST_ADDR + 0x220;

        let read = read_for_device(&harness.entity, CPU_A_DEVICE_ID, addr);
        let read_response = read
            .clone()
            .with_routing(CACHE_A_DEVICE_ID, DIRECTORY_DEVICE_ID)
            .with_access_type(AccessType::ReadResponse)
            .with_coherence_op(Some(CoherenceOp::GrantShared));
        let invalidate = invalidate(
            &harness.entity,
            addr,
            CACHE_A_DEVICE_ID,
            DIRECTORY_DEVICE_ID,
        );

        let shared_read_txn = MemoryTxn::read_req(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::SharedRead));
        let read_response_txn = MemoryTxn::read_rsp(addr);
        let invalidate_ack_txn = MemoryTxn::control(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::InvalidateAck));

        harness.run_steps([
            send_dev_rx!(read.clone()),
            expect_mem_tx!(shared_read_txn.clone()),
            send_mem_rx!(read_response),
            expect_dev_tx!(read_response_txn),
            send_mem_rx!(invalidate),
            expect_mem_tx!(invalidate_ack_txn),
            send_dev_rx!(read),
            expect_mem_tx!(shared_read_txn),
        ]);
    }

    #[test]
    fn coherent_cache_dirty_invalidate_writes_back_before_ack() {
        let coherency_manager_memory_map =
            MemoryMap::from_regions(&[(0u64, u64::MAX, DIRECTORY_DEVICE_ID)]).unwrap();
        let mut engine = start_test(file!());
        let cache = create_coherent_cache(&mut engine, coherency_manager_memory_map);
        let mut harness = CacheHarness::<MemoryAccess>::new(engine, cache);
        let addr = DST_ADDR + 0x240;

        let write = write_for_device(&harness.entity, CPU_A_DEVICE_ID, addr);
        let grant_exclusive = write
            .clone()
            .with_routing(CACHE_A_DEVICE_ID, DIRECTORY_DEVICE_ID)
            .with_access_type(AccessType::Control)
            .with_coherence_op(Some(CoherenceOp::GrantExclusive));
        let invalidate = invalidate(&harness.entity, addr, CACHE_A_DEVICE_ID, CPU_A_DEVICE_ID);

        let exclusive_write_txn = MemoryTxn::write_req(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::ExclusiveWrite));
        let writeback_txn = MemoryTxn::write_req(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(None);
        let invalidate_ack_txn = MemoryTxn::control(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(CPU_A_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::InvalidateAck));

        harness.run_steps([
            send_dev_rx!(write),
            expect_mem_tx!(exclusive_write_txn),
            send_mem_rx!(grant_exclusive),
            send_dev_rx!(invalidate),
            expect_mem_tx!(writeback_txn),
            expect_dev_tx!(invalidate_ack_txn),
        ]);
    }

    #[test]
    fn coherent_cache_memory_side_dirty_invalidate_writes_back_before_ack() {
        let coherency_manager_memory_map =
            MemoryMap::from_regions(&[(0u64, u64::MAX, DIRECTORY_DEVICE_ID)]).unwrap();
        let mut engine = start_test(file!());
        let cache = create_coherent_cache(&mut engine, coherency_manager_memory_map);
        let mut harness = CacheHarness::<MemoryAccess>::new(engine, cache);
        let addr = DST_ADDR + 0x250;

        let write = write_for_device(&harness.entity, CPU_A_DEVICE_ID, addr);
        let grant_exclusive = write
            .clone()
            .with_routing(CACHE_A_DEVICE_ID, DIRECTORY_DEVICE_ID)
            .with_access_type(AccessType::Control)
            .with_coherence_op(Some(CoherenceOp::GrantExclusive));
        let invalidate = invalidate(
            &harness.entity,
            addr,
            CACHE_A_DEVICE_ID,
            DIRECTORY_DEVICE_ID,
        );

        let exclusive_write_txn = MemoryTxn::write_req(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::ExclusiveWrite));
        let writeback_txn = MemoryTxn::write_req(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(None);
        let invalidate_ack_txn = MemoryTxn::control(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::InvalidateAck));

        harness.run_steps([
            send_dev_rx!(write),
            expect_mem_tx!(exclusive_write_txn),
            send_mem_rx!(grant_exclusive),
            send_mem_rx!(invalidate),
            expect_mem_tx!(writeback_txn),
            expect_mem_tx!(invalidate_ack_txn),
        ]);
    }

    #[test]
    fn coherent_cache_barrier_waits_for_outstanding_fill_and_blocks_later_requests() {
        let coherency_manager_memory_map =
            MemoryMap::from_regions(&[(0u64, u64::MAX, DIRECTORY_DEVICE_ID)]).unwrap();
        let mut engine = start_test(file!());
        let cache = create_coherent_cache(&mut engine, coherency_manager_memory_map);
        let mut harness = CacheHarness::<MemoryAccess>::new(engine, cache.clone());
        let addr = DST_ADDR + 0x260;

        let read = read_for_device(&harness.entity, CPU_A_DEVICE_ID, addr);
        let barrier = barrier(&harness.entity, addr, CPU_A_DEVICE_ID);
        let read_response = read
            .clone()
            .with_routing(CACHE_A_DEVICE_ID, DIRECTORY_DEVICE_ID)
            .with_access_type(AccessType::ReadResponse)
            .with_coherence_op(Some(CoherenceOp::GrantShared));
        let barrier_response = barrier
            .clone()
            .with_routing(CACHE_A_DEVICE_ID, DIRECTORY_DEVICE_ID)
            .with_access_type(AccessType::BarrierResponse);

        let shared_read_txn = MemoryTxn::read_req(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::SharedRead));
        let read_response_txn = MemoryTxn::read_rsp(addr);
        let barrier_txn = MemoryTxn::barrier_req(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID);
        let barrier_response_txn = MemoryTxn::barrier_rsp(addr);

        harness.run_steps([
            send_dev_rx!(read.clone()),
            expect_mem_tx!(shared_read_txn),
            send_dev_rx!(barrier),
            send_dev_rx!(read),
            delay!(10),
            send_mem_rx!(read_response),
            expect_dev_tx!(read_response_txn.clone()),
            expect_mem_tx!(barrier_txn),
            send_mem_rx!(barrier_response),
            expect_dev_tx!(barrier_response_txn),
            expect_dev_tx!(read_response_txn),
        ]);

        assert_eq!(cache.num_hits(), 1);
        assert_eq!(cache.num_misses(), 1);
    }

    #[test]
    fn coherent_read_grant_exclusive_enables_local_write_hit() {
        let coherency_manager_memory_map =
            MemoryMap::from_regions(&[(0u64, u64::MAX, DIRECTORY_DEVICE_ID)]).unwrap();
        let mut engine = start_test(file!());
        let cache = create_coherent_cache(&mut engine, coherency_manager_memory_map);
        let mut harness = CacheHarness::new(engine, cache.clone());
        let addr = DST_ADDR + 0x280;

        let read = read_for_device(&harness.entity, CPU_A_DEVICE_ID, addr);
        let grant_exclusive_read_response = read
            .clone()
            .with_routing(CACHE_A_DEVICE_ID, DIRECTORY_DEVICE_ID)
            .with_access_type(AccessType::ReadResponse)
            .with_coherence_op(Some(CoherenceOp::GrantExclusive));
        let write = write_for_device(&harness.entity, CPU_A_DEVICE_ID, addr);

        let shared_read_txn = MemoryTxn::read_req(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::SharedRead));
        let read_response_txn = MemoryTxn::read_rsp(addr);

        harness.run_steps([
            send_dev_rx!(read),
            expect_mem_tx!(shared_read_txn),
            send_mem_rx!(grant_exclusive_read_response),
            expect_dev_tx!(read_response_txn),
            send_dev_rx!(write),
            expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
        ]);

        assert_eq!(cache.num_misses(), 1);
        assert_eq!(cache.num_hits(), 1);
    }

    #[test]
    fn coherent_grant_exclusive_read_response_with_pending_write_becomes_modified() {
        let coherency_manager_memory_map =
            MemoryMap::from_regions(&[(0u64, u64::MAX, DIRECTORY_DEVICE_ID)]).unwrap();
        let mut engine = start_test(file!());
        let cache = create_coherent_cache(&mut engine, coherency_manager_memory_map);
        let mut harness = CacheHarness::new(engine, cache.clone());
        let addr = DST_ADDR + 0x2a0;

        let read = read_for_device(&harness.entity, CPU_A_DEVICE_ID, addr);
        let write = write_for_device(&harness.entity, CPU_A_DEVICE_ID, addr);
        let grant_exclusive_read_response = read
            .clone()
            .with_routing(CACHE_A_DEVICE_ID, DIRECTORY_DEVICE_ID)
            .with_access_type(AccessType::ReadResponse)
            .with_coherence_op(Some(CoherenceOp::GrantExclusive));

        let shared_read_txn = MemoryTxn::read_req(addr)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::SharedRead));
        let read_response_txn = MemoryTxn::read_rsp(addr);

        harness.run_steps([
            send_dev_rx!(read),
            expect_mem_tx!(shared_read_txn),
            send_dev_rx!(write.clone()),
            expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS / 2) as u64),
            send_mem_rx!(grant_exclusive_read_response),
            expect_dev_tx!(read_response_txn),
            expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
            send_dev_rx!(write),
            expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
        ]);

        assert_eq!(cache.num_misses(), 1);
        assert_eq!(cache.num_hits(), 2);
    }

    #[test]
    fn pending_write_same_set_retry_stays_queued_while_line_is_still_allocated() {
        let coherency_manager_memory_map =
            MemoryMap::from_regions(&[(0u64, u64::MAX, DIRECTORY_DEVICE_ID)]).unwrap();
        let mut engine = start_test(file!());
        let cache = create_coherent_cache(&mut engine, coherency_manager_memory_map);
        let mut harness = CacheHarness::new(engine, cache);
        let same_set_stride = (CACHE_CAPACITY_BYTES / NUM_WAYS) as u64;
        let addr_a = DST_ADDR + 0x300;
        let addr_b = addr_a + same_set_stride;

        let write_b = write_for_device(&harness.entity, CPU_A_DEVICE_ID, addr_b);
        let read_a = read_for_device(&harness.entity, CPU_A_DEVICE_ID, addr_a);
        let read_a_response = read_a
            .clone()
            .with_routing(CACHE_A_DEVICE_ID, DIRECTORY_DEVICE_ID)
            .with_access_type(AccessType::ReadResponse)
            .with_coherence_op(Some(CoherenceOp::GrantShared));

        let exclusive_write_b_txn = MemoryTxn::write_req(addr_b)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::ExclusiveWrite));
        let shared_read_a_txn = MemoryTxn::read_req(addr_a)
            .with_src_device(CACHE_A_DEVICE_ID)
            .with_dst_device(DIRECTORY_DEVICE_ID)
            .with_coherence_op(Some(CoherenceOp::SharedRead));
        let read_a_response_txn = MemoryTxn::read_rsp(addr_a);

        harness.run_steps([
            send_dev_rx!(write_b.clone()),
            expect_mem_tx!(exclusive_write_b_txn),
            send_dev_rx!(write_b),
            send_dev_rx!(read_a),
            expect_mem_tx!(shared_read_a_txn),
            send_mem_rx!(read_a_response),
            expect_dev_tx!(read_a_response_txn),
            expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
        ]);
    }
}

struct TwoCacheSystem<T>
where
    T: SimObject + Routable + CoherentAccess + AccessMemory,
{
    #[allow(dead_code)]
    fabric: Rc<FunctionalFabric<T>>,
    manager: Rc<CoherencyManager>,
    cache_a: Rc<Cache<T>>,
    cache_b: Rc<Cache<T>>,
    memory: Rc<Memory<T>>,
}

impl<T> TwoCacheSystem<T>
where
    T: SimObject + Routable + CoherentAccess + AccessMemory,
{
    fn cache(&self, index: usize) -> &Rc<Cache<T>> {
        match index {
            0 => &self.cache_a,
            1 => &self.cache_b,
            _ => panic!("invalid cache index {index}"),
        }
    }

    fn port_dev_rx_i(&self, index: usize) -> PortStateResult<T> {
        self.cache(index).port_dev_rx()
    }

    fn connect_port_dev_tx_i(&self, index: usize, port_state: PortStateResult<T>) -> SimResult {
        self.cache(index).connect_port_dev_tx(port_state)
    }
}

impl TwoCacheSystem<MemoryAccess> {
    fn new(engine: &mut Engine) -> Rc<Self> {
        const CAPACITY_BYTES: usize = CACHE_CAPACITY_BYTES * NUM_WAYS * 2;

        let clock = engine.default_clock();
        let top = engine.top();

        let fabric_config = Rc::new(FabricConfig::new(1, 4, 1, None, 1, 1, 64, 64, 1024));
        let fabric =
            FunctionalFabric::new_and_register(engine, &clock, top, "fabric", fabric_config)
                .unwrap();

        let memory_map =
            Rc::new(MemoryMap::from_regions(&[(0u64, u64::MAX, DIRECTORY_DEVICE_ID)]).unwrap());
        let cache_a = Cache::new_and_register(
            engine,
            &clock,
            top,
            "cache_a",
            create_cache_config(CACHE_A_DEVICE_ID).with_coherency_manager_memory_map(&memory_map),
        )
        .unwrap();

        let cache_b = Cache::new_and_register(
            engine,
            &clock,
            top,
            "cache_b",
            create_cache_config(CACHE_B_DEVICE_ID).with_coherency_manager_memory_map(&memory_map),
        )
        .unwrap();

        let coherent_memory_map = MemoryMap::from_regions(&[(
            BASE_ADDRESS,
            CAPACITY_BYTES as u64,
            BACKING_MEMORY_DEVICE_ID,
        )])
        .unwrap();

        let manager = CoherencyManager::new_and_register(
            engine,
            &clock,
            top,
            "directory",
            CoherencyManagerConfig::new(LINE_SIZE_BYTES, DIRECTORY_DEVICE_ID, coherent_memory_map),
        )
        .unwrap();

        let memory = Memory::new_and_register(
            engine,
            &clock,
            top,
            "memory",
            MemoryConfig::new(
                BASE_ADDRESS,
                CAPACITY_BYTES,
                BW_BYTES_PER_CYCLE,
                DELAY_TICKS,
            ),
        )
        .unwrap();

        connect_port!(cache_a, mem_tx => fabric, ingress, 0).unwrap();
        connect_port!(fabric, egress, 0 => cache_a, mem_rx).unwrap();
        connect_port!(cache_b, mem_tx => fabric, ingress, 1).unwrap();
        connect_port!(fabric, egress, 1 => cache_b, mem_rx).unwrap();
        connect_port!(manager, tx => fabric, ingress, 2).unwrap();
        connect_port!(fabric, egress, 2 => manager, rx).unwrap();
        connect_port!(memory, tx => fabric, ingress, 3).unwrap();
        connect_port!(fabric, egress, 3 => memory, rx).unwrap();

        Rc::new(Self {
            fabric,
            manager,
            cache_a,
            cache_b,
            memory,
        })
    }
}

fn read_a(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    read_for_device(created_by, CPU_A_DEVICE_ID, addr)
}

fn read_b(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    read_for_device(created_by, CPU_B_DEVICE_ID, addr)
}

fn write_a(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    write_for_device(created_by, CPU_A_DEVICE_ID, addr)
}

fn write_np_a(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    write_np_for_device(created_by, CPU_A_DEVICE_ID, addr)
}

fn barrier_a(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    barrier(created_by, addr, CPU_A_DEVICE_ID)
}

mod two_cache_harness {
    use super::*;

    build_model_harness! {
        harness TwoCacheHarness<T> {
            component: system: Rc<TwoCacheSystem<T>>,
            rx port arrays: {
                DevRx<T> => dev_rx {
                    count: num_dev_rx
                }
            },
            tx port arrays: {
                DevTx<T> => dev_tx {
                    count: num_dev_tx
                }
            }
        }
    }

    #[test]
    fn coherent_two_caches_hold_shared_reads() {
        let mut engine = start_test(file!());
        let system = TwoCacheSystem::new(&mut engine);
        let mut harness =
            TwoCacheHarness::new(engine, system.clone(), NUM_DEV_PORTS, NUM_DEV_PORTS);
        let addr = DST_ADDR + 0x80;

        let cache_a_read = read_a(&harness.entity, addr);
        let cache_b_read = read_b(&harness.entity, addr);

        let read_a_response_txn = MemoryTxn::read_rsp(addr);
        let read_b_response_txn = MemoryTxn::read_rsp(addr);

        harness.run_steps([
            par!([
                send_dev_rx!(0, cache_a_read),
                expect_dev_tx!(0, read_a_response_txn),
            ]),
            par!([
                send_dev_rx!(1, cache_b_read),
                expect_dev_tx!(1, read_b_response_txn),
            ]),
        ]);

        assert_eq!(system.cache_a.num_misses(), 1);
        assert_eq!(system.cache_b.num_misses(), 1);
        assert_eq!(system.memory.bytes_read(), 2 * ACCESS_SIZE_BYTES);
        assert_eq!(system.manager.total_received_count(), 2);
        assert_eq!(system.manager.total_sent_count(), 2);
    }

    #[test]
    fn coherent_write_invalidates_other_cache_and_reread_observes_update_order() {
        let mut engine = start_test(file!());
        let system = TwoCacheSystem::new(&mut engine);
        let mut harness =
            TwoCacheHarness::new(engine, system.clone(), NUM_DEV_PORTS, NUM_DEV_PORTS);
        let addr = DST_ADDR + 0x100;
        let read_a = read_a(&harness.entity, addr);
        let read_b = read_b(&harness.entity, addr);
        let write_a = write_a(&harness.entity, addr);
        let barrier_a = barrier_a(&harness.entity, addr);

        let read_a_response_txn = MemoryTxn::read_rsp(addr);
        let read_b_response_txn = MemoryTxn::read_rsp(addr);
        let barrier_a_response_txn = MemoryTxn::barrier_rsp(addr);

        harness.run_steps([
            par!([
                send_dev_rx!(0, read_a.clone()),
                expect_dev_tx!(0, read_a_response_txn.clone()),
            ]),
            par!([
                send_dev_rx!(1, read_b.clone()),
                expect_dev_tx!(1, read_b_response_txn.clone()),
            ]),
            par!([
                seq!([
                    send_dev_rx!(0, write_a),
                    // Ensure the write has completed with a barrier
                    send_dev_rx!(0, barrier_a),
                ]),
                expect_dev_tx!(0, barrier_a_response_txn),
            ]),
            par!([
                send_dev_rx!(0, read_a),
                expect_dev_tx!(0, read_a_response_txn),
            ]),
            par!([
                send_dev_rx!(1, read_b),
                expect_dev_tx!(1, read_b_response_txn),
            ]),
        ]);

        assert_eq!(system.memory.bytes_written(), ACCESS_SIZE_BYTES);
    }

    #[test]
    fn coherent_write_hit_after_grant_stays_local_and_ordered() {
        let mut engine = start_test(file!());
        let system = TwoCacheSystem::new(&mut engine);
        let mut harness =
            TwoCacheHarness::new(engine, system.clone(), NUM_DEV_PORTS, NUM_DEV_PORTS);
        let addr = DST_ADDR + 0x180;

        let read = read_a(&harness.entity, addr);
        let write = write_a(&harness.entity, addr);
        let barrier = barrier_a(&harness.entity, addr);

        let read_response_txn = MemoryTxn::read_rsp(addr);
        let barrier_response_txn = MemoryTxn::barrier_rsp(addr);

        harness.run_steps([
            par!([
                send_dev_rx!(0, read.clone()),
                expect_dev_tx!(0, read_response_txn.clone()),
            ]),
            par!([
                seq!([send_dev_rx!(0, write.clone()), send_dev_rx!(0, barrier),]),
                expect_dev_tx!(0, barrier_response_txn),
            ]),
            par!([
                seq!([send_dev_rx!(0, write), send_dev_rx!(0, read)]),
                expect_dev_tx!(0, read_response_txn),
            ]),
        ]);

        assert_eq!(system.memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(system.memory.bytes_written(), 0);
    }

    #[test]
    fn coherent_nonposted_write_completes_locally_and_stays_dirty_until_invalidated() {
        let mut engine = start_test(file!());
        let system = TwoCacheSystem::new(&mut engine);
        let mut harness =
            TwoCacheHarness::new(engine, system.clone(), NUM_DEV_PORTS, NUM_DEV_PORTS);
        let addr = DST_ADDR + 0x1a0;

        let read = read_a(&harness.entity, addr);
        let write_np = write_np_a(&harness.entity, addr);
        let barrier = barrier_a(&harness.entity, addr);

        let read_response_txn = MemoryTxn::read_rsp(addr);
        let write_np_response_txn = MemoryTxn::write_np_rsp(addr);
        let barrier_response_txn = MemoryTxn::barrier_rsp(addr);

        harness.run_steps([par!([
            seq!([
                send_dev_rx!(0, read.clone()),
                send_dev_rx!(0, write_np),
                send_dev_rx!(0, barrier),
                send_dev_rx!(0, read),
            ]),
            seq!([
                expect_dev_tx!(0, read_response_txn.clone()),
                expect_dev_tx!(0, write_np_response_txn),
                expect_dev_tx!(0, barrier_response_txn),
                expect_dev_tx!(0, read_response_txn),
            ]),
        ])]);

        assert_eq!(system.memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(system.memory.bytes_written(), 0);
    }

    #[test]
    fn coherent_nonposted_write_hit_completes_locally() {
        let mut engine = start_test(file!());
        let system = TwoCacheSystem::new(&mut engine);
        let mut harness =
            TwoCacheHarness::new(engine, system.clone(), NUM_DEV_PORTS, NUM_DEV_PORTS);
        let addr = DST_ADDR + 0x1c0;

        let read = read_a(&harness.entity, addr);
        let write_np = write_np_a(&harness.entity, addr);
        let barrier = barrier_a(&harness.entity, addr);

        let read_response_txn = MemoryTxn::read_rsp(addr);
        let write_np_response_txn = MemoryTxn::write_np_rsp(addr);
        let barrier_response_txn = MemoryTxn::barrier_rsp(addr);

        harness.run_steps([par!([
            seq!([
                send_dev_rx!(0, read),
                send_dev_rx!(0, write_np.clone()),
                send_dev_rx!(0, barrier.clone()),
                send_dev_rx!(0, write_np),
                send_dev_rx!(0, barrier),
            ]),
            seq!([
                expect_dev_tx!(0, read_response_txn),
                expect_dev_tx!(0, write_np_response_txn.clone()),
                expect_dev_tx!(0, barrier_response_txn.clone()),
                expect_dev_tx!(0, write_np_response_txn),
                expect_dev_tx!(0, barrier_response_txn),
            ]),
        ])]);

        assert_eq!(system.memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(system.memory.bytes_written(), 0);
    }

    #[test]
    fn coherent_same_line_read_write_read_is_deterministic() {
        let mut engine = start_test(file!());
        let system = TwoCacheSystem::new(&mut engine);
        let mut harness =
            TwoCacheHarness::new(engine, system.clone(), NUM_DEV_PORTS, NUM_DEV_PORTS);
        let addr = DST_ADDR + 0x1c0;

        let read = read_a(&harness.entity, addr);
        let write = write_a(&harness.entity, addr);
        let barrier = barrier_a(&harness.entity, addr);

        let read_response_txn = MemoryTxn::read_rsp(addr);
        let barrier_response_txn = MemoryTxn::barrier_rsp(addr);

        harness.run_steps([par!([
            seq!([
                send_dev_rx!(0, read.clone()),
                send_dev_rx!(0, write),
                send_dev_rx!(0, barrier),
                send_dev_rx!(0, read),
            ]),
            seq!([
                expect_dev_tx!(0, read_response_txn.clone()),
                expect_dev_tx!(0, barrier_response_txn),
                expect_dev_tx!(0, read_response_txn),
            ]),
        ])]);

        assert_eq!(system.memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(system.memory.bytes_written(), 0);
    }
}
