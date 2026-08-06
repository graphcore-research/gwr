// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_components::connect_port;
use gwr_engine::engine::Engine;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::SimObject;
use gwr_engine::types::AccessType;
use gwr_models::build_model_harness;
use gwr_models::cache::coherency_manager::CoherenceOp;
use gwr_models::cache::traits::CoherentAccess;
use gwr_models::cache::{Cache, CacheConfig, CacheHintType};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::{DeviceId, MemoryMap};
use gwr_models::memory::traits::{AccessMemory, ReadMemory};
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::MemoryTxn;
use gwr_track::entity::Entity;

const BASE_ADDR: u64 = 0x80000;
const SRC_ADDR: u64 = BASE_ADDR + 0x1000;
const ACCESS_SIZE_BYTES: usize = 32;
const OVERHEAD_SIZE_BYTES: usize = 16;
const BW_BYTES_PER_CYCLE: usize = 8;
const LINE_SIZE_BYTES: usize = ACCESS_SIZE_BYTES;
const NUM_SETS: usize = 16;
const NUM_WAYS: usize = 2;
const CACHE_CAPACITY_BYTES: usize = NUM_SETS * NUM_WAYS * LINE_SIZE_BYTES;
const DELAY_TICKS: usize = 20;

const CACHE_DEVICE_ID: DeviceId = DeviceId(0);
const DIRECTORY_DEVICE_ID: DeviceId = DeviceId(2);
const MEMORY_DEVICE_ID: DeviceId = DeviceId(3);
const SECOND_MEMORY_DEVICE_ID: DeviceId = DeviceId(4);
const CPU_DEVICE_ID: DeviceId = DeviceId(10);

fn create_cache(engine: &mut Engine, config: CacheConfig) -> Rc<Cache<MemoryAccess>> {
    let clock = engine.default_clock();
    Cache::new_and_register(engine, &clock, engine.top(), "cache", config).unwrap()
}

fn cache_config() -> CacheConfig {
    let memory_map = Rc::new(MemoryMap::from_regions(&[(0, u64::MAX, MEMORY_DEVICE_ID)]).unwrap());
    CacheConfig::new(
        CACHE_DEVICE_ID,
        LINE_SIZE_BYTES,
        BW_BYTES_PER_CYCLE,
        NUM_SETS,
        NUM_WAYS,
        DELAY_TICKS,
        &memory_map,
    )
}

fn create_and_connect_memory<T>(engine: &mut Engine, cache: &Rc<Cache<T>>) -> Rc<Memory<T>>
where
    T: SimObject + CoherentAccess,
{
    let clock = engine.default_clock();
    let top = engine.top();

    let config = MemoryConfig::new(
        BASE_ADDR,
        CACHE_CAPACITY_BYTES * NUM_WAYS * 2,
        BW_BYTES_PER_CYCLE,
        DELAY_TICKS,
    );
    let memory = Memory::new_and_register(engine, &clock, top, "memory", config).unwrap();

    connect_port!(cache, mem_tx => memory, rx).unwrap();
    connect_port!(memory, tx => cache, mem_rx).unwrap();

    memory
}

fn read_from_device(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::ReadRequest,
        ACCESS_SIZE_BYTES,
        addr,
        SRC_ADDR,
        MEMORY_DEVICE_ID,
        CPU_DEVICE_ID,
        OVERHEAD_SIZE_BYTES,
    )
}

fn write_from_device(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::WriteRequest,
        ACCESS_SIZE_BYTES,
        addr,
        SRC_ADDR,
        MEMORY_DEVICE_ID,
        CPU_DEVICE_ID,
        OVERHEAD_SIZE_BYTES,
    )
}

fn write_np_from_device(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    write_from_device(created_by, addr).with_access_type(AccessType::WriteNonPostedRequest)
}

fn barrier_from_device(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::BarrierRequest,
        0,
        addr,
        0,
        DIRECTORY_DEVICE_ID,
        CPU_DEVICE_ID,
        OVERHEAD_SIZE_BYTES,
    )
}

fn control_from_device(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::Control,
        ACCESS_SIZE_BYTES,
        addr,
        SRC_ADDR,
        CACHE_DEVICE_ID,
        CPU_DEVICE_ID,
        OVERHEAD_SIZE_BYTES,
    )
    .with_coherence_op(Some(CoherenceOp::Invalidate))
}

fn invalidate_from_manager(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::Control,
        ACCESS_SIZE_BYTES,
        addr,
        SRC_ADDR,
        CACHE_DEVICE_ID,
        DIRECTORY_DEVICE_ID,
        OVERHEAD_SIZE_BYTES,
    )
    .with_coherence_op(Some(CoherenceOp::Invalidate))
}

fn grant_exclusive(created_by: &Rc<Entity>, addr: u64) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::Control,
        ACCESS_SIZE_BYTES,
        addr,
        SRC_ADDR,
        CACHE_DEVICE_ID,
        DIRECTORY_DEVICE_ID,
        OVERHEAD_SIZE_BYTES,
    )
    .with_coherence_op(Some(CoherenceOp::GrantExclusive))
}

fn grant_shared_read_response(request: &MemoryAccess) -> MemoryAccess {
    request
        .clone()
        .with_routing(CACHE_DEVICE_ID, DIRECTORY_DEVICE_ID)
        .with_access_type(AccessType::ReadResponse)
        .with_coherence_op(Some(CoherenceOp::GrantShared))
}

fn grant_exclusive_read_response(request: &MemoryAccess) -> MemoryAccess {
    request
        .clone()
        .with_routing(CACHE_DEVICE_ID, DIRECTORY_DEVICE_ID)
        .with_access_type(AccessType::ReadResponse)
        .with_coherence_op(Some(CoherenceOp::GrantExclusive))
}

fn coherency_txn(access_type: AccessType, addr: u64, op: CoherenceOp) -> MemoryTxn {
    MemoryTxn::new(access_type, addr)
        .with_src_device(CACHE_DEVICE_ID)
        .with_dst_device(DIRECTORY_DEVICE_ID)
        .with_coherence_op(Some(op))
        .with_src_addr(SRC_ADDR)
        .with_bytes(ACCESS_SIZE_BYTES)
}

fn same_set_stride() -> u64 {
    (LINE_SIZE_BYTES * NUM_SETS) as u64
}

/// Test the basics of the cache by driving/handling all of the ports manually
mod cache_test {
    use super::*;

    struct EmptyMemory;

    impl ReadMemory for EmptyMemory {
        fn read(&self) -> Vec<u8> {
            Vec::new()
        }
    }

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

    fn coherent_cache_config() -> CacheConfig {
        cache_config().with_coherency_manager_memory_map(&Rc::new(
            MemoryMap::from_regions(&[(0u64, u64::MAX, DIRECTORY_DEVICE_ID)]).unwrap(),
        ))
    }

    fn noncoherent_harness() -> CacheHarness<MemoryAccess> {
        let mut engine = start_test(file!());
        let cache = create_cache(&mut engine, cache_config());
        CacheHarness::new(engine, cache)
    }

    fn coherent_harness() -> CacheHarness<MemoryAccess> {
        let mut engine = start_test(file!());
        let cache = create_cache(&mut engine, coherent_cache_config());
        CacheHarness::new(engine, cache)
    }

    mod port_contracts {
        use super::*;

        #[test]
        fn device_read_request_uses_mem_tx_and_memory_read_response_uses_dev_tx() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR;

            let read = read_from_device(&harness.entity, addr);
            let read_response = read.to_response(&EmptyMemory).unwrap();

            let read_txn = MemoryTxn::read_req(addr)
                .with_src_addr(SRC_ADDR)
                .with_bytes(ACCESS_SIZE_BYTES);
            let read_response_txn = MemoryTxn::read_rsp(addr)
                .with_src_addr(SRC_ADDR)
                .with_bytes(ACCESS_SIZE_BYTES);

            harness.run_steps([
                send_dev_rx!(read),
                expect_mem_tx!(read_txn),
                send_mem_rx!(read_response),
                expect_dev_tx!(read_response_txn),
            ]);
        }

        #[test]
        fn device_write_nonposted_request_and_memory_response_are_forwarded() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x40;

            let write_np = write_np_from_device(&harness.entity, addr);
            let write_np_response = write_np.to_response(&EmptyMemory).unwrap();

            let write_np_txn = MemoryTxn::write_np_req(addr)
                .with_src_addr(SRC_ADDR)
                .with_bytes(ACCESS_SIZE_BYTES);
            let write_np_response_txn = MemoryTxn::write_np_rsp(addr)
                .with_src_addr(SRC_ADDR)
                .with_bytes(ACCESS_SIZE_BYTES);

            harness.run_steps([
                send_dev_rx!(write_np),
                expect_mem_tx!(write_np_txn),
                send_mem_rx!(write_np_response),
                expect_dev_tx!(write_np_response_txn),
            ]);
        }

        #[test]
        fn device_acks_invalidate_request_and_refetches_data() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x80;

            let read = read_from_device(&harness.entity, addr);
            let read_response = read.to_response(&EmptyMemory).unwrap();
            let invalidate = control_from_device(&harness.entity, addr);

            let read_txn = MemoryTxn::read_req(addr);
            let read_response_txn = MemoryTxn::read_rsp(addr);
            let invalidate_ack_txn = MemoryTxn::control(addr)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(CPU_DEVICE_ID)
                .with_coherence_op(Some(CoherenceOp::InvalidateAck));

            harness.run_steps([
                send_dev_rx!(read.clone()),
                expect_mem_tx!(read_txn.clone()),
                send_mem_rx!(read_response),
                expect_dev_tx!(read_response_txn),
                send_dev_rx!(invalidate),
                par!([
                    expect_dev_tx!(invalidate_ack_txn),
                    expect_no_traffic!(&[Port::MemTx], (DELAY_TICKS * 2) as u64),
                ]),
                send_dev_rx!(read),
                expect_mem_tx!(read_txn),
            ]);
        }

        #[test]
        fn coherent_device_barrier_uses_mem_tx_and_memory_barrier_response_uses_dev_tx() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0xc0;

            let barrier = barrier_from_device(&harness.entity, addr);
            let barrier_response = barrier
                .clone()
                .with_routing(CACHE_DEVICE_ID, DIRECTORY_DEVICE_ID)
                .with_access_type(AccessType::BarrierResponse);

            let barrier_txn = MemoryTxn::barrier_req(addr)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(DIRECTORY_DEVICE_ID);
            let barrier_response_txn = MemoryTxn::barrier_rsp(addr);

            harness.run_steps([
                send_dev_rx!(barrier),
                expect_mem_tx!(barrier_txn),
                send_mem_rx!(barrier_response),
                expect_dev_tx!(barrier_response_txn),
            ]);
        }

        #[test]
        fn incoherent_device_barrier_completes_locally_without_mem_traffic() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0xe0;

            let barrier = barrier_from_device(&harness.entity, addr);
            let barrier_response_txn = MemoryTxn::barrier_rsp(addr);

            harness.run_steps([
                send_dev_rx!(barrier),
                par!([
                    expect_dev_tx!(barrier_response_txn),
                    expect_no_traffic!(&[Port::MemTx], (DELAY_TICKS * 2) as u64),
                ]),
            ]);
        }

        #[test]
        fn memory_control_invalidate_forces_writeback_before_ack() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x100;

            let write = write_from_device(&harness.entity, addr);
            let grant_exclusive = grant_exclusive(&harness.entity, addr);
            let invalidate = invalidate_from_manager(&harness.entity, addr);

            let exclusive_write_txn =
                coherency_txn(AccessType::WriteRequest, addr, CoherenceOp::ExclusiveWrite);
            let writeback_txn = MemoryTxn::write_req(addr)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(DIRECTORY_DEVICE_ID)
                .with_coherence_op(None);
            let invalidate_ack_txn = MemoryTxn::control(addr)
                .with_src_device(CACHE_DEVICE_ID)
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
    }

    mod error_paths {
        use super::*;

        #[test]
        #[should_panic(expected = "unsupported AccessType from device: ReadResponse")]
        fn device_response_access_type_returns_error() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x140;

            let read_response = read_from_device(&harness.entity, addr)
                .to_response(&EmptyMemory)
                .unwrap();

            harness.run_steps([send_dev_rx!(read_response)]);
        }

        #[test]
        #[should_panic(expected = "unsupported coherence op None on device control access")]
        fn device_control_without_coherence_op_returns_error() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x180;

            let control = control_from_device(&harness.entity, addr).with_coherence_op(None);

            harness.run_steps([send_dev_rx!(control)]);
        }

        #[test]
        #[should_panic(
            expected = "unsupported coherence op Some(GrantExclusive) on device control access"
        )]
        fn device_control_with_unsupported_coherence_op_returns_error() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x1c0;

            let control = control_from_device(&harness.entity, addr)
                .with_coherence_op(Some(CoherenceOp::GrantExclusive));

            harness.run_steps([send_dev_rx!(control)]);
        }

        #[test]
        #[should_panic(expected = "unsupported ReadRequest on response port")]
        fn memory_request_access_type_returns_error() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x200;

            let read = read_from_device(&harness.entity, addr)
                .with_routing(CACHE_DEVICE_ID, MEMORY_DEVICE_ID);

            harness.run_steps([send_mem_rx!(read)]);
        }

        #[test]
        #[should_panic(expected = "unsupported coherence op None on memory control access")]
        fn memory_control_without_coherence_op_returns_error() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x240;

            let control = invalidate_from_manager(&harness.entity, addr).with_coherence_op(None);

            harness.run_steps([send_mem_rx!(control)]);
        }

        #[test]
        #[should_panic(
            expected = "unsupported coherence op Some(GrantShared) on memory control access"
        )]
        fn memory_control_with_unsupported_coherence_op_returns_error() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x280;

            let control = invalidate_from_manager(&harness.entity, addr)
                .with_coherence_op(Some(CoherenceOp::GrantShared));

            harness.run_steps([send_mem_rx!(control)]);
        }
    }

    mod noncoherent_line_state_transitions {
        use super::*;

        #[test]
        fn invalid_to_allocated_shared_to_shared_then_read_hit() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x200;

            let read = read_from_device(&harness.entity, addr);
            let read_response = read.to_response(&EmptyMemory).unwrap();

            let read_txn = MemoryTxn::read_req(addr);
            let read_response_txn = MemoryTxn::read_rsp(addr);

            harness.run_steps([
                send_dev_rx!(read.clone()),
                expect_mem_tx!(read_txn),
                send_dev_rx!(read.clone()),
                delay!(1),
                send_mem_rx!(read_response),
                expect_dev_tx!(read_response_txn.clone()),
                expect_dev_tx!(read_response_txn.clone()),
                send_dev_rx!(read),
                par!([
                    expect_dev_tx!(read_response_txn),
                    expect_no_traffic!(&[Port::MemTx], (DELAY_TICKS * 2) as u64),
                ]),
            ]);
        }

        #[test]
        fn shared_to_invalid_then_reread_allocates_again() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x240;

            let read = read_from_device(&harness.entity, addr);
            let read_response = read.to_response(&EmptyMemory).unwrap();
            let invalidate = control_from_device(&harness.entity, addr);

            let read_txn = MemoryTxn::read_req(addr);
            let read_response_txn = MemoryTxn::read_rsp(addr);
            let invalidate_ack_txn = MemoryTxn::control(addr)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(CPU_DEVICE_ID)
                .with_coherence_op(Some(CoherenceOp::InvalidateAck));

            harness.run_steps([
                send_dev_rx!(read.clone()),
                expect_mem_tx!(read_txn.clone()),
                send_mem_rx!(read_response),
                expect_dev_tx!(read_response_txn),
                send_dev_rx!(invalidate),
                par!([
                    expect_dev_tx!(invalidate_ack_txn),
                    expect_no_traffic!(&[Port::MemTx], (DELAY_TICKS * 2) as u64),
                ]),
                send_dev_rx!(read),
                expect_mem_tx!(read_txn),
            ]);
        }

        #[test]
        fn noallocate_read_miss_does_not_fill_line() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x280;

            let noallocate_read =
                read_from_device(&harness.entity, addr).with_cache_hint(CacheHintType::NoAllocate);
            let noallocate_read_response = noallocate_read.to_response(&EmptyMemory).unwrap();
            let allocate_read = read_from_device(&harness.entity, addr);

            let noallocate_read_txn =
                MemoryTxn::read_req(addr).with_cache_hint(CacheHintType::NoAllocate);
            let noallocate_read_response_txn =
                MemoryTxn::read_rsp(addr).with_cache_hint(CacheHintType::NoAllocate);
            let allocate_read_txn = MemoryTxn::read_req(addr);

            harness.run_steps([
                send_dev_rx!(noallocate_read),
                expect_mem_tx!(noallocate_read_txn),
                send_mem_rx!(noallocate_read_response),
                expect_dev_tx!(noallocate_read_response_txn),
                send_dev_rx!(allocate_read),
                expect_mem_tx!(allocate_read_txn),
            ]);
        }

        #[test]
        fn noallocate_nonposted_write_does_not_fill_line() {
            let mut harness = noncoherent_harness();
            let addr = BASE_ADDR + 0x2a0;

            let noallocate_write_np = write_np_from_device(&harness.entity, addr)
                .with_cache_hint(CacheHintType::NoAllocate);
            let noallocate_write_np_response =
                noallocate_write_np.to_response(&EmptyMemory).unwrap();
            let allocate_read = read_from_device(&harness.entity, addr);

            let noallocate_write_np_txn =
                MemoryTxn::write_np_req(addr).with_cache_hint(CacheHintType::NoAllocate);
            let noallocate_write_np_response_txn =
                MemoryTxn::write_np_rsp(addr).with_cache_hint(CacheHintType::NoAllocate);
            let allocate_read_txn = MemoryTxn::read_req(addr);

            harness.run_steps([
                send_dev_rx!(noallocate_write_np),
                expect_mem_tx!(noallocate_write_np_txn),
                send_mem_rx!(noallocate_write_np_response),
                expect_dev_tx!(noallocate_write_np_response_txn),
                send_dev_rx!(allocate_read),
                expect_mem_tx!(allocate_read_txn),
            ]);
        }

        #[test]
        fn allocated_lines_are_not_evicted_until_a_way_is_freed() {
            let mut harness = noncoherent_harness();
            let stride = same_set_stride();

            let addrs: Vec<u64> = (0..=NUM_WAYS)
                .map(|i| BASE_ADDR + 0x400 + (i as u64 * stride))
                .collect();

            let mut requests: Vec<_> = addrs
                .iter()
                .map(|addr| read_from_device(&harness.entity, *addr))
                .collect();
            let first_read_response = requests[0].to_response(&EmptyMemory).unwrap();

            let mut read_txns: Vec<_> = addrs
                .iter()
                .take(NUM_WAYS)
                .map(|addr| MemoryTxn::read_req(*addr))
                .collect();
            let first_read_response_txn = MemoryTxn::read_rsp(addrs[0]);
            let stalled_read_txn = MemoryTxn::read_req(addrs[NUM_WAYS]);

            let mut steps = Vec::new();
            for request in requests.drain(..) {
                steps.push(send_dev_rx!(request));
            }
            for read_txn in read_txns.drain(..) {
                steps.push(expect_mem_tx!(read_txn));
            }
            steps.push(expect_no_traffic!(
                &[Port::DevTx, Port::MemTx],
                (DELAY_TICKS * 2) as u64,
            ));
            steps.push(send_mem_rx!(first_read_response));
            steps.push(par!([
                expect_dev_tx!(first_read_response_txn),
                expect_mem_tx!(stalled_read_txn),
            ]));

            harness.run_steps(steps);
        }

        #[test]
        fn eviction_writeback_uses_evicted_address_memory_map() {
            let mut engine = start_test(file!());
            let addr_a = BASE_ADDR + 0x600;
            let addr_b = addr_a + same_set_stride();
            let memory_map = Rc::new(
                MemoryMap::from_regions(&[
                    (addr_a, LINE_SIZE_BYTES as u64, MEMORY_DEVICE_ID),
                    (addr_b, LINE_SIZE_BYTES as u64, SECOND_MEMORY_DEVICE_ID),
                ])
                .unwrap(),
            );
            let config = CacheConfig::new(
                CACHE_DEVICE_ID,
                LINE_SIZE_BYTES,
                BW_BYTES_PER_CYCLE,
                NUM_SETS,
                1,
                DELAY_TICKS,
                &memory_map,
            );
            let cache = create_cache(&mut engine, config);
            let mut harness = CacheHarness::new(engine, cache);

            let write_b = write_from_device(&harness.entity, addr_b)
                .with_routing(SECOND_MEMORY_DEVICE_ID, CPU_DEVICE_ID);
            let read_a = read_from_device(&harness.entity, addr_a)
                .with_routing(MEMORY_DEVICE_ID, CPU_DEVICE_ID);

            let write_b_txn = MemoryTxn::write_req(addr_b).with_dst_device(SECOND_MEMORY_DEVICE_ID);
            let writeback_b_txn = MemoryTxn::write_req(addr_b)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(SECOND_MEMORY_DEVICE_ID)
                .with_coherence_op(None);
            let read_a_txn = MemoryTxn::read_req(addr_a).with_dst_device(MEMORY_DEVICE_ID);

            harness.run_steps([
                send_dev_rx!(write_b),
                expect_mem_tx!(write_b_txn),
                send_dev_rx!(read_a),
                expect_mem_tx!(writeback_b_txn),
                expect_mem_tx!(read_a_txn),
            ]);
        }
    }

    mod coherent_line_state_transitions {
        use super::*;

        #[test]
        fn invalid_to_allocated_exclusive_to_modified_for_posted_write() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x280;

            let write = write_from_device(&harness.entity, addr);
            let grant_exclusive = grant_exclusive(&harness.entity, addr);
            let read = read_from_device(&harness.entity, addr);

            let exclusive_write_txn =
                coherency_txn(AccessType::WriteRequest, addr, CoherenceOp::ExclusiveWrite);
            let read_response_txn = MemoryTxn::read_rsp(addr);

            harness.run_steps([
                send_dev_rx!(write),
                expect_mem_tx!(exclusive_write_txn),
                send_mem_rx!(grant_exclusive),
                send_dev_rx!(read),
                par!([
                    expect_dev_tx!(read_response_txn),
                    expect_no_traffic!(&[Port::MemTx], (DELAY_TICKS * 2) as u64),
                ]),
            ]);
        }

        #[test]
        fn grant_exclusive_read_response_without_pending_write_creates_exclusive_line() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x2c0;

            let read = read_from_device(&harness.entity, addr);
            let grant_exclusive_read_response = grant_exclusive_read_response(&read);
            let write = write_from_device(&harness.entity, addr);

            let shared_read_txn =
                coherency_txn(AccessType::ReadRequest, addr, CoherenceOp::SharedRead);
            let read_response_txn = MemoryTxn::read_rsp(addr);

            harness.run_steps([
                send_dev_rx!(read),
                expect_mem_tx!(shared_read_txn),
                send_mem_rx!(grant_exclusive_read_response),
                expect_dev_tx!(read_response_txn),
                send_dev_rx!(write),
                expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
            ]);
        }

        #[test]
        fn noallocate_read_miss_does_not_fill_coherent_line() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x2e0;

            let noallocate_read =
                read_from_device(&harness.entity, addr).with_cache_hint(CacheHintType::NoAllocate);
            let grant_shared_noallocate_response = grant_shared_read_response(&noallocate_read);
            let allocate_read = read_from_device(&harness.entity, addr);

            let noallocate_shared_read_txn =
                coherency_txn(AccessType::ReadRequest, addr, CoherenceOp::SharedRead)
                    .with_cache_hint(CacheHintType::NoAllocate);
            let noallocate_read_response_txn =
                MemoryTxn::read_rsp(addr).with_cache_hint(CacheHintType::NoAllocate);
            let allocate_shared_read_txn =
                coherency_txn(AccessType::ReadRequest, addr, CoherenceOp::SharedRead);

            harness.run_steps([
                send_dev_rx!(noallocate_read),
                expect_mem_tx!(noallocate_shared_read_txn),
                send_mem_rx!(grant_shared_noallocate_response),
                expect_dev_tx!(noallocate_read_response_txn),
                send_dev_rx!(allocate_read),
                expect_mem_tx!(allocate_shared_read_txn),
            ]);
        }

        #[test]
        fn noallocate_nonposted_write_does_not_fill_coherent_line() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x2f0;

            let noallocate_write_np = write_np_from_device(&harness.entity, addr)
                .with_cache_hint(CacheHintType::NoAllocate);
            let grant_exclusive = noallocate_write_np
                .to_response(&EmptyMemory)
                .unwrap()
                .with_coherence_op(Some(CoherenceOp::GrantExclusive))
                .with_cache_hint(CacheHintType::NoAllocate);
            let allocate_read = read_from_device(&harness.entity, addr);

            let noallocate_exclusive_write_txn = coherency_txn(
                AccessType::WriteNonPostedRequest,
                addr,
                CoherenceOp::ExclusiveWrite,
            )
            .with_cache_hint(CacheHintType::NoAllocate);
            let noallocate_write_np_response_txn =
                MemoryTxn::write_np_rsp(addr).with_cache_hint(CacheHintType::NoAllocate);
            let allocate_shared_read_txn =
                coherency_txn(AccessType::ReadRequest, addr, CoherenceOp::SharedRead);

            harness.run_steps([
                send_dev_rx!(noallocate_write_np),
                expect_mem_tx!(noallocate_exclusive_write_txn),
                send_mem_rx!(grant_exclusive),
                expect_dev_tx!(noallocate_write_np_response_txn),
                send_dev_rx!(allocate_read),
                expect_mem_tx!(allocate_shared_read_txn),
            ]);
        }

        #[test]
        fn shared_to_allocated_exclusive_to_modified_for_nonposted_write_invalidate_writeback() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x300;

            let read = read_from_device(&harness.entity, addr);
            let grant_shared_read_response = grant_shared_read_response(&read);
            let write_np = write_np_from_device(&harness.entity, addr);
            let grant_exclusive = grant_exclusive(&harness.entity, addr);
            let invalidate = invalidate_from_manager(&harness.entity, addr);

            let shared_read_txn =
                coherency_txn(AccessType::ReadRequest, addr, CoherenceOp::SharedRead);
            let read_response_txn = MemoryTxn::read_rsp(addr);
            let exclusive_write_txn = coherency_txn(
                AccessType::WriteNonPostedRequest,
                addr,
                CoherenceOp::ExclusiveWrite,
            );
            let write_np_response_txn = MemoryTxn::write_np_rsp(addr);
            let writeback_txn = MemoryTxn::write_req(addr)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(DIRECTORY_DEVICE_ID)
                .with_coherence_op(None);
            let invalidate_ack_txn = MemoryTxn::control(addr)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(DIRECTORY_DEVICE_ID)
                .with_coherence_op(Some(CoherenceOp::InvalidateAck));

            harness.run_steps([
                send_dev_rx!(read),
                expect_mem_tx!(shared_read_txn),
                send_mem_rx!(grant_shared_read_response),
                expect_dev_tx!(read_response_txn),
                send_dev_rx!(write_np),
                expect_mem_tx!(exclusive_write_txn),
                send_mem_rx!(grant_exclusive),
                expect_dev_tx!(write_np_response_txn),
                send_mem_rx!(invalidate),
                expect_mem_tx!(writeback_txn),
                expect_mem_tx!(invalidate_ack_txn),
            ]);
        }

        #[test]
        fn nonposted_write_response_grant_exclusive_completes_pending_write() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x340;

            let write_np = write_np_from_device(&harness.entity, addr);
            let grant_exclusive_write_np_response = write_np
                .to_response(&EmptyMemory)
                .unwrap()
                .with_coherence_op(Some(CoherenceOp::GrantExclusive));
            let read = read_from_device(&harness.entity, addr);

            let exclusive_write_txn = coherency_txn(
                AccessType::WriteNonPostedRequest,
                addr,
                CoherenceOp::ExclusiveWrite,
            );
            let write_np_response_txn = MemoryTxn::write_np_rsp(addr);
            let read_response_txn = MemoryTxn::read_rsp(addr);

            harness.run_steps([
                send_dev_rx!(write_np),
                expect_mem_tx!(exclusive_write_txn),
                send_mem_rx!(grant_exclusive_write_np_response),
                expect_dev_tx!(write_np_response_txn),
                send_dev_rx!(read),
                par!([
                    expect_dev_tx!(read_response_txn),
                    expect_no_traffic!(&[Port::MemTx], (DELAY_TICKS * 2) as u64),
                ]),
            ]);
        }

        #[test]
        fn pending_read_behind_nonposted_write_grant_completes_after_write_response() {
            let mut harness = coherent_harness();
            let addr = BASE_ADDR + 0x380;

            let write_np = write_np_from_device(&harness.entity, addr);
            let grant_exclusive_write_np_response = write_np
                .to_response(&EmptyMemory)
                .unwrap()
                .with_coherence_op(Some(CoherenceOp::GrantExclusive));
            let read = read_from_device(&harness.entity, addr);

            let exclusive_write_txn = coherency_txn(
                AccessType::WriteNonPostedRequest,
                addr,
                CoherenceOp::ExclusiveWrite,
            );
            let write_np_response_txn = MemoryTxn::write_np_rsp(addr);
            let read_response_txn = MemoryTxn::read_rsp(addr);

            harness.run_steps([
                send_dev_rx!(write_np),
                expect_mem_tx!(exclusive_write_txn),
                send_dev_rx!(read),
                send_mem_rx!(grant_exclusive_write_np_response),
                expect_dev_tx!(write_np_response_txn),
                expect_dev_tx!(read_response_txn),
            ]);
        }

        #[test]
        fn read_miss_writes_back_modified_victim_before_reallocation() {
            let mut harness = coherent_harness();
            let stride = same_set_stride();
            let addr_a = BASE_ADDR + 0x500;
            let addr_b = addr_a + stride;
            let addr_c = addr_a + (2 * stride);

            let write_a = write_from_device(&harness.entity, addr_a);
            let grant_exclusive_a = grant_exclusive(&harness.entity, addr_a);
            let write_b = write_from_device(&harness.entity, addr_b);
            let grant_exclusive_b = grant_exclusive(&harness.entity, addr_b);
            let read_c = read_from_device(&harness.entity, addr_c);

            let exclusive_write_a_txn = coherency_txn(
                AccessType::WriteRequest,
                addr_a,
                CoherenceOp::ExclusiveWrite,
            );
            let exclusive_write_b_txn = coherency_txn(
                AccessType::WriteRequest,
                addr_b,
                CoherenceOp::ExclusiveWrite,
            );
            let writeback_a_txn = MemoryTxn::write_req(addr_a)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(DIRECTORY_DEVICE_ID)
                .with_coherence_op(None);
            let shared_read_c_txn =
                coherency_txn(AccessType::ReadRequest, addr_c, CoherenceOp::SharedRead);

            harness.run_steps([
                send_dev_rx!(write_a),
                expect_mem_tx!(exclusive_write_a_txn),
                send_mem_rx!(grant_exclusive_a),
                send_dev_rx!(write_b),
                expect_mem_tx!(exclusive_write_b_txn),
                send_mem_rx!(grant_exclusive_b),
                send_dev_rx!(read_c),
                expect_mem_tx!(writeback_a_txn),
                expect_mem_tx!(shared_read_c_txn),
            ]);
        }

        #[test]
        fn miss_blocks_when_lru_way_is_allocated_even_if_non_lru_way_is_modified() {
            let mut harness = coherent_harness();
            let stride = same_set_stride();
            let addr_a = BASE_ADDR + 0x600;
            let addr_b = addr_a + stride;
            let addr_c = addr_a + (2 * stride);

            let read_a = read_from_device(&harness.entity, addr_a);
            let write_b = write_from_device(&harness.entity, addr_b);
            let grant_exclusive_b = grant_exclusive(&harness.entity, addr_b);
            let read_c = read_from_device(&harness.entity, addr_c);

            let shared_read_a_txn =
                coherency_txn(AccessType::ReadRequest, addr_a, CoherenceOp::SharedRead);
            let exclusive_write_b_txn = coherency_txn(
                AccessType::WriteRequest,
                addr_b,
                CoherenceOp::ExclusiveWrite,
            );

            harness.run_steps([
                send_dev_rx!(read_a),
                expect_mem_tx!(shared_read_a_txn),
                send_dev_rx!(write_b),
                expect_mem_tx!(exclusive_write_b_txn),
                send_mem_rx!(grant_exclusive_b),
                send_dev_rx!(read_c),
                expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
            ]);
        }

        #[test]
        fn write_miss_blocks_when_lru_way_is_allocated_even_if_non_lru_way_is_modified() {
            let mut harness = coherent_harness();
            let stride = same_set_stride();
            let addr_a = BASE_ADDR + 0x680;
            let addr_b = addr_a + stride;
            let addr_c = addr_a + (2 * stride);

            let read_a = read_from_device(&harness.entity, addr_a);
            let write_b = write_from_device(&harness.entity, addr_b);
            let grant_exclusive_b = grant_exclusive(&harness.entity, addr_b);
            let write_c = write_from_device(&harness.entity, addr_c);

            let shared_read_a_txn =
                coherency_txn(AccessType::ReadRequest, addr_a, CoherenceOp::SharedRead);
            let exclusive_write_b_txn = coherency_txn(
                AccessType::WriteRequest,
                addr_b,
                CoherenceOp::ExclusiveWrite,
            );

            harness.run_steps([
                send_dev_rx!(read_a),
                expect_mem_tx!(shared_read_a_txn),
                send_dev_rx!(write_b),
                expect_mem_tx!(exclusive_write_b_txn),
                send_mem_rx!(grant_exclusive_b),
                send_dev_rx!(write_c),
                expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
            ]);
        }

        #[test]
        fn pending_read_retry_blocks_when_same_set_still_has_no_evictable_way() {
            let mut harness = coherent_harness();
            let stride = same_set_stride();
            let addr_a = BASE_ADDR + 0x700;
            let addr_b = addr_a + stride;
            let addr_c = addr_a + (2 * stride);
            let addr_d = addr_a + (3 * stride);

            let read_a = read_from_device(&harness.entity, addr_a);
            let write_b = write_from_device(&harness.entity, addr_b);
            let grant_exclusive_b = grant_exclusive(&harness.entity, addr_b);
            let read_c = read_from_device(&harness.entity, addr_c);
            let invalidate_d = invalidate_from_manager(&harness.entity, addr_d);

            let shared_read_a_txn =
                coherency_txn(AccessType::ReadRequest, addr_a, CoherenceOp::SharedRead);
            let exclusive_write_b_txn = coherency_txn(
                AccessType::WriteRequest,
                addr_b,
                CoherenceOp::ExclusiveWrite,
            );
            let invalidate_d_ack_txn = MemoryTxn::control(addr_d)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(DIRECTORY_DEVICE_ID)
                .with_coherence_op(Some(CoherenceOp::InvalidateAck));

            harness.run_steps([
                send_dev_rx!(read_a),
                expect_mem_tx!(shared_read_a_txn),
                send_dev_rx!(write_b),
                expect_mem_tx!(exclusive_write_b_txn),
                send_mem_rx!(grant_exclusive_b),
                send_dev_rx!(read_c),
                expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
                send_mem_rx!(invalidate_d),
                expect_mem_tx!(invalidate_d_ack_txn),
                expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
            ]);
        }

        #[test]
        fn pending_write_retry_blocks_when_same_set_still_has_no_evictable_way() {
            let mut harness = coherent_harness();
            let stride = same_set_stride();
            let addr_a = BASE_ADDR + 0x780;
            let addr_b = addr_a + stride;
            let addr_c = addr_a + (2 * stride);
            let addr_d = addr_a + (3 * stride);

            let read_a = read_from_device(&harness.entity, addr_a);
            let write_b = write_from_device(&harness.entity, addr_b);
            let grant_exclusive_b = grant_exclusive(&harness.entity, addr_b);
            let write_c = write_from_device(&harness.entity, addr_c);
            let invalidate_d = invalidate_from_manager(&harness.entity, addr_d);

            let shared_read_a_txn =
                coherency_txn(AccessType::ReadRequest, addr_a, CoherenceOp::SharedRead);
            let exclusive_write_b_txn = coherency_txn(
                AccessType::WriteRequest,
                addr_b,
                CoherenceOp::ExclusiveWrite,
            );
            let invalidate_d_ack_txn = MemoryTxn::control(addr_d)
                .with_src_device(CACHE_DEVICE_ID)
                .with_dst_device(DIRECTORY_DEVICE_ID)
                .with_coherence_op(Some(CoherenceOp::InvalidateAck));

            harness.run_steps([
                send_dev_rx!(read_a),
                expect_mem_tx!(shared_read_a_txn),
                send_dev_rx!(write_b),
                expect_mem_tx!(exclusive_write_b_txn),
                send_mem_rx!(grant_exclusive_b),
                send_dev_rx!(write_c),
                expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
                send_mem_rx!(invalidate_d),
                expect_mem_tx!(invalidate_d_ack_txn),
                expect_no_traffic!(&[Port::DevTx, Port::MemTx], (DELAY_TICKS * 2) as u64),
            ]);
        }
    }
}

mod memory_integration {
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

    fn cache_with_memory_harness() -> (
        CacheDevHarness<MemoryAccess>,
        Rc<Cache<MemoryAccess>>,
        Rc<Memory<MemoryAccess>>,
    ) {
        let mut engine = start_test(file!());
        let cache = create_cache(&mut engine, cache_config());
        let memory = create_and_connect_memory(&mut engine, &cache);
        let harness = CacheDevHarness::new(engine, cache.clone());

        (harness, cache, memory)
    }

    #[test]
    fn cache_connected_to_memory_serves_repeated_read_hits_locally() {
        let (mut harness, cache, memory) = cache_with_memory_harness();

        let num_reads = 10;
        let addr = BASE_ADDR + 0x800;

        let read = read_from_device(&harness.entity, addr);
        let read_response_txn = MemoryTxn::read_rsp(addr)
            .with_src_addr(SRC_ADDR)
            .with_bytes(ACCESS_SIZE_BYTES);

        let mut reads = Vec::new();
        let mut responses = Vec::new();
        for _ in 0..num_reads {
            reads.push(send_dev_rx!(read.clone()));
            responses.push(expect_dev_tx!(read_response_txn.clone()));
        }

        harness.run_steps([par!([seq!(reads), seq!(responses)])]);

        assert_eq!(cache.payload_bytes_read(), num_reads * ACCESS_SIZE_BYTES);
        assert_eq!(cache.payload_bytes_written(), 0);
        assert_eq!(cache.num_misses(), 1);
        assert_eq!(cache.num_hits(), num_reads - 1);
        assert_eq!(memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_written(), 0);
    }

    #[test]
    fn cache_connected_to_memory_hits_across_all_ways() {
        let (mut harness, cache, memory) = cache_with_memory_harness();

        let num_iterations = 10;
        let reads: Vec<_> = (0..NUM_WAYS)
            .map(|i| {
                let addr = BASE_ADDR + 0x900 + (i * CACHE_CAPACITY_BYTES / NUM_WAYS) as u64;
                (addr, read_from_device(&harness.entity, addr))
            })
            .collect();
        let read_response_txns: Vec<_> = reads
            .iter()
            .map(|(addr, _)| {
                MemoryTxn::read_rsp(*addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES)
            })
            .collect();

        let mut steps = Vec::new();
        for _ in 0..num_iterations {
            for ((_, read), read_response_txn) in reads.iter().zip(read_response_txns.iter()) {
                steps.push(send_dev_rx!(read.clone()));
                steps.push(expect_dev_tx!(read_response_txn.clone()));
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

    #[test]
    fn cache_connected_to_memory_thrashes_when_set_exceeds_ways() {
        let (mut harness, cache, memory) = cache_with_memory_harness();

        let num_iterations = 10;
        let reads: Vec<_> = (0..NUM_WAYS + 1)
            .map(|i| {
                let addr = BASE_ADDR + 0xa00 + (i * CACHE_CAPACITY_BYTES / NUM_WAYS) as u64;
                (addr, read_from_device(&harness.entity, addr))
            })
            .collect();
        let read_response_txns: Vec<_> = reads
            .iter()
            .map(|(addr, _)| {
                MemoryTxn::read_rsp(*addr)
                    .with_src_addr(SRC_ADDR)
                    .with_bytes(ACCESS_SIZE_BYTES)
            })
            .collect();

        let mut steps = Vec::new();
        for _ in 0..num_iterations {
            for ((_, read), read_response_txn) in reads.iter().zip(read_response_txns.iter()) {
                steps.push(send_dev_rx!(read.clone()));
                steps.push(expect_dev_tx!(read_response_txn.clone()));
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

    #[test]
    fn cache_connected_to_memory_write_allocates_line_and_serves_reread_hits() {
        let (mut harness, cache, memory) = cache_with_memory_harness();

        let num_rereads = 3;
        let addr = BASE_ADDR + 0xb00;
        let read = read_from_device(&harness.entity, addr);
        let write = write_from_device(&harness.entity, addr);
        let read_response_txn = MemoryTxn::read_rsp(addr)
            .with_src_addr(SRC_ADDR)
            .with_bytes(ACCESS_SIZE_BYTES);

        let mut steps = Vec::new();
        for _ in 0..num_rereads {
            steps.push(send_dev_rx!(read.clone()));
            steps.push(expect_dev_tx!(read_response_txn.clone()));
        }

        steps.push(send_dev_rx!(write));

        for _ in 0..num_rereads {
            steps.push(send_dev_rx!(read.clone()));
            steps.push(expect_dev_tx!(read_response_txn.clone()));
        }

        harness.run_steps(steps);

        assert_eq!(
            cache.payload_bytes_read(),
            num_rereads * 2 * ACCESS_SIZE_BYTES
        );
        assert_eq!(cache.payload_bytes_written(), ACCESS_SIZE_BYTES);
        assert_eq!(cache.num_misses(), 2);
        assert_eq!(cache.num_hits(), (num_rereads * 2) - 1);
        assert_eq!(memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(memory.bytes_written(), ACCESS_SIZE_BYTES);
    }
}
