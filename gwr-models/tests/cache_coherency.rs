// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_components::connect_port;
use gwr_components::test_helpers::expect_value;
use gwr_engine::engine::Engine;
use gwr_engine::port::PortStateResult;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::{Routable, SimObject};
use gwr_engine::types::{AccessType, DeviceId, SimResult};
use gwr_models::build_model_harness;
use gwr_models::cache::coherency_manager::{CoherenceOp, CoherencyManager, CoherencyManagerConfig};
use gwr_models::cache::traits::CoherentAccess;
use gwr_models::cache::{Cache, CacheConfig, CoherenceConfig};
use gwr_models::fabric::functional::FunctionalFabric;
use gwr_models::fabric::{Fabric, FabricConfig, FabricGeometry, FabricPortConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::memory::traits::AccessMemory;
use gwr_models::memory::{CacheHintType, Memory, MemoryConfig};
use gwr_models::test_helpers::MemoryTxn;
use gwr_track::entity::Entity;

const BASE_ADDRESS: u64 = 0x80000;
const DST_ADDR: u64 = BASE_ADDRESS;
const SRC_ADDR: u64 = BASE_ADDRESS + 0x1000;

const BW_BYTES_PER_CYCLE: usize = 8;
const LINE_SIZE_BYTES: usize = 32;
const CACHE_NUM_SETS: usize = 1;
const CACHE_NUM_WAYS: usize = 1;

const ACCESS_SIZE_BYTES: usize = LINE_SIZE_BYTES;
const OVERHEAD_SIZE_BYTES: usize = 16;
const BACKING_MEMORY_CAPACITY_BYTES: usize = 1024 * 1024;
const DELAY_TICKS: usize = 20;
const CACHE_A_DEVICE_ID: DeviceId = DeviceId(0);
const CACHE_B_DEVICE_ID: DeviceId = DeviceId(1);
const DIRECTORY_DEVICE_ID: DeviceId = DeviceId(2);
const BACKING_MEMORY_DEVICE_ID: DeviceId = DeviceId(3);
const CPU_A_DEVICE_ID: DeviceId = DeviceId(10);
const CPU_B_DEVICE_ID: DeviceId = DeviceId(11);
const NUM_CLIENT_PORTS: usize = CACHE_B_CLIENT_PORT + 1;
const CACHE_A_CLIENT_PORT: usize = 0;
const CACHE_B_CLIENT_PORT: usize = 1;
const CACHE_A_FABRIC_PORT: usize = 0;
const CACHE_B_FABRIC_PORT: usize = 1;
const DIRECTORY_FABRIC_PORT: usize = 2;
const BACKING_MEMORY_FABRIC_PORT: usize = 3;
const NUM_FABRIC_PORTS: usize = BACKING_MEMORY_FABRIC_PORT + 1;
const FABRIC_BUFFER_SIZE_BYTES: usize = 256;
const FABRIC_BANDWIDTH_BITS_PER_TICK: usize = 1024;

mod two_cache_harness {
    use super::*;

    build_model_harness! {
        harness TwoCacheHarness<T: CoherentAccess> {
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
        let (mut harness, system) = create_harness(false);
        let address = DST_ADDR + 0x80;

        let cache_a_read_request = build_read_request_for_cache_a(harness.engine.top(), address);
        let cache_b_read_request = build_read_request_for_cache_b(harness.engine.top(), address);

        let expected_cache_a_read_response = MemoryTxn::read_rsp(address);
        let expected_cache_b_read_response = MemoryTxn::read_rsp(address);

        // Populate a shared copy of the same line in each cache.
        harness.run_steps([
            par!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request.clone()),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response.clone()),
            ]),
            par!([
                send_dev_rx!(CACHE_B_CLIENT_PORT, cache_b_read_request.clone()),
                expect_dev_tx!(CACHE_B_CLIENT_PORT, expected_cache_b_read_response.clone()),
            ]),
            // Reread both caches after both shared copies have been installed.
            par!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response),
            ]),
            par!([
                send_dev_rx!(CACHE_B_CLIENT_PORT, cache_b_read_request),
                expect_dev_tx!(CACHE_B_CLIENT_PORT, expected_cache_b_read_response),
            ]),
        ]);

        // Each cache retains its shared copy: one miss, then one local hit.
        assert_eq!(system.cache_a.num_hits(), 1);
        assert_eq!(system.cache_b.num_hits(), 1);
        assert_eq!(system.cache_a.num_misses(), 1);
        assert_eq!(system.cache_b.num_misses(), 1);

        // Only the initial fills reach backing memory and the directory.
        assert_eq!(system.memory.bytes_read(), 2 * ACCESS_SIZE_BYTES);
        assert_eq!(system.manager.received_coherence_message_count(), 2);
        assert_eq!(system.manager.sent_coherence_message_count(), 2);
    }

    #[test]
    fn coherent_write_invalidates_other_cache_and_reread_observes_update_order() {
        let (mut harness, system) = create_harness(false);
        let address = DST_ADDR + 0x100;
        let cache_a_read_request = build_read_request_for_cache_a(harness.engine.top(), address);
        let cache_b_read_request = build_read_request_for_cache_b(harness.engine.top(), address);
        let cache_a_write_request = build_write_request_for_cache_a(harness.engine.top(), address);
        let cache_a_barrier_request =
            build_barrier_request_for_cache_a(harness.engine.top(), address);

        let expected_cache_a_read_response = MemoryTxn::read_rsp(address);
        let expected_cache_b_read_response = MemoryTxn::read_rsp(address);
        let expected_cache_a_barrier_response = MemoryTxn::barrier_rsp(address);
        let memory_clone = system.memory.clone();

        harness.run_steps([
            // Populate a shared copy in each cache.
            par!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request.clone()),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response.clone()),
            ]),
            par!([
                send_dev_rx!(CACHE_B_CLIENT_PORT, cache_b_read_request.clone()),
                expect_dev_tx!(CACHE_B_CLIENT_PORT, expected_cache_b_read_response.clone()),
            ]),
            // Upgrade cache A and drain the write before continuing.
            par!([
                seq!([
                    send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_write_request),
                    // Ensure the write has completed with a barrier
                    send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_barrier_request),
                ]),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_barrier_response),
            ]),
            // Reread both copies after the write has completed.
            par!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response),
            ]),
            script! {
                ports {
                    cache_b_requests = DevRx[CACHE_B_CLIENT_PORT],
                    cache_b_responses = DevTx[CACHE_B_CLIENT_PORT],
                }
                async {
                    // The local write and reread must leave the dirty line in cache A.
                    assert_eq!(memory_clone.bytes_written(), 0);

                    // Cache B's reread must resolve that dirty owner before replying.
                    cache_b_requests.put(cache_b_read_request)?.await;
                    expect_value(
                        &mut cache_b_responses,
                        &expected_cache_b_read_response,
                        "cache B reread response",
                    ).await?;

                    // Exactly one whole-line writeback must have reached memory.
                    assert_eq!(memory_clone.bytes_written(), LINE_SIZE_BYTES);
                    Ok(())
                }
            },
        ]);
    }

    #[test]
    fn coherent_write_hit_after_grant_stays_local_and_ordered() {
        let (mut harness, system) = create_harness(false);
        let address = DST_ADDR + 0x180;

        let cache_a_read_request = build_read_request_for_cache_a(harness.engine.top(), address);
        let cache_a_write_request = build_write_request_for_cache_a(harness.engine.top(), address);
        let cache_a_barrier_request =
            build_barrier_request_for_cache_a(harness.engine.top(), address);

        let expected_cache_a_read_response = MemoryTxn::read_rsp(address);
        let expected_cache_a_barrier_response = MemoryTxn::barrier_rsp(address);

        harness.run_steps([
            // Read the line into cache A.
            par!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request.clone()),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response.clone()),
            ]),
            // Acquire write permission and wait for the first write.
            par!([
                seq!([
                    send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_write_request.clone()),
                    send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_barrier_request),
                ]),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_barrier_response),
            ]),
            // Write and reread the resident exclusive line.
            par!([
                seq!([
                    send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_write_request),
                    send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request)
                ]),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response),
            ]),
        ]);

        // The initial read and permission upgrade miss; the later write and
        // read hit.
        assert_eq!(system.cache_a.num_misses(), 2);
        assert_eq!(system.cache_a.num_hits(), 2);

        // Repeated writes stay local after the initial line read.
        assert_eq!(system.memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(system.memory.bytes_written(), 0);
    }

    #[test]
    fn coherent_nonposted_write_completes_locally_and_stays_dirty_until_invalidated() {
        let (mut harness, system) = create_harness(false);
        let address = DST_ADDR + 0x1a0;

        let cache_a_read_request = build_read_request_for_cache_a(harness.engine.top(), address);
        let cache_a_nonposted_write_request =
            build_nonposted_write_request_for_cache_a(harness.engine.top(), address);
        let cache_a_barrier_request =
            build_barrier_request_for_cache_a(harness.engine.top(), address);

        let expected_cache_a_read_response = MemoryTxn::read_rsp(address);
        let expected_cache_a_nonposted_write_response = MemoryTxn::write_np_rsp(address);
        let expected_cache_a_barrier_response = MemoryTxn::barrier_rsp(address);

        // Read, write, drain, and reread the same resident line.
        harness.run_steps([par!([
            seq!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request.clone()),
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_nonposted_write_request),
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_barrier_request),
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request),
            ]),
            seq!([
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response.clone()),
                expect_dev_tx!(
                    CACHE_A_CLIENT_PORT,
                    expected_cache_a_nonposted_write_response
                ),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_barrier_response),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response),
            ]),
        ])]);

        // A local write completion does not flush the dirty line.
        assert_eq!(system.memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(system.memory.bytes_written(), 0);
    }

    #[test]
    fn coherent_nonposted_write_hit_completes_locally() {
        let (mut harness, system) = create_harness(false);
        let address = DST_ADDR + 0x1c0;

        let cache_a_read_request = build_read_request_for_cache_a(harness.engine.top(), address);
        let cache_a_nonposted_write_request =
            build_nonposted_write_request_for_cache_a(harness.engine.top(), address);
        let cache_a_barrier_request =
            build_barrier_request_for_cache_a(harness.engine.top(), address);

        let expected_cache_a_read_response = MemoryTxn::read_rsp(address);
        let expected_cache_a_nonposted_write_response = MemoryTxn::write_np_rsp(address);
        let expected_cache_a_barrier_response = MemoryTxn::barrier_rsp(address);

        // Repeat an acknowledged write after permission has been acquired.
        harness.run_steps([par!([
            seq!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request),
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_nonposted_write_request.clone()),
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_barrier_request.clone()),
                // Repeat the write after the first barrier has completed.
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_nonposted_write_request),
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_barrier_request),
            ]),
            seq!([
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response),
                expect_dev_tx!(
                    CACHE_A_CLIENT_PORT,
                    expected_cache_a_nonposted_write_response.clone()
                ),
                expect_dev_tx!(
                    CACHE_A_CLIENT_PORT,
                    expected_cache_a_barrier_response.clone()
                ),
                // The second resident write also receives a local response.
                expect_dev_tx!(
                    CACHE_A_CLIENT_PORT,
                    expected_cache_a_nonposted_write_response
                ),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_barrier_response),
            ]),
        ])]);

        // Both writes complete without backing-memory writes.
        assert_eq!(system.memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(system.memory.bytes_written(), 0);
    }

    #[test]
    fn coherent_same_line_read_write_read_is_deterministic() {
        let (mut harness, system) = create_harness(false);
        let address = DST_ADDR + 0x1c0;

        let cache_a_read_request = build_read_request_for_cache_a(harness.engine.top(), address);
        let cache_a_write_request = build_write_request_for_cache_a(harness.engine.top(), address);
        let cache_a_barrier_request =
            build_barrier_request_for_cache_a(harness.engine.top(), address);

        let expected_cache_a_read_response = MemoryTxn::read_rsp(address);
        let expected_cache_a_barrier_response = MemoryTxn::barrier_rsp(address);

        // Order a same-line read, write, barrier, and final read.
        harness.run_steps([par!([
            seq!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request.clone()),
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_write_request),
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_barrier_request),
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request),
            ]),
            seq!([
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response.clone()),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_barrier_response),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, expected_cache_a_read_response),
            ]),
        ])]);

        // The final read hits the locally updated dirty line.
        assert_eq!(system.memory.bytes_read(), ACCESS_SIZE_BYTES);
        assert_eq!(system.memory.bytes_written(), 0);
    }

    #[test]
    fn coherent_multiline_read_streams_and_returns_original_range() {
        let (mut harness, system) = create_harness(false);
        let cache_a_read_request = MemoryAccess::new(
            harness.engine.top(),
            AccessType::ReadRequest,
            3 * LINE_SIZE_BYTES - 1,
            DST_ADDR + 1,
            SRC_ADDR,
            BACKING_MEMORY_DEVICE_ID,
            CPU_A_DEVICE_ID,
            OVERHEAD_SIZE_BYTES,
        );

        // Read an unaligned range across three lines.
        harness.run_steps([par!([
            send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_read_request),
            expect_dev_tx!(
                CACHE_A_CLIENT_PORT,
                MemoryTxn::read_rsp(DST_ADDR + 1).with_bytes(3 * LINE_SIZE_BYTES - 1)
            )
        ])]);

        // Fetch whole lines while releasing the single fill slot after each
        // grant.
        assert_eq!(system.memory.bytes_read(), 3 * LINE_SIZE_BYTES);
        assert_eq!(system.cache_a.traffic_snapshot().fill_requests, 3);
        assert_eq!(system.cache_a.inflight_fills_snapshot().peak, 1);
        assert_eq!(system.cache_a.inflight_fills_snapshot().current, 0);
    }

    #[test]
    fn noallocate_multiline_write_forwards_exact_ranges() {
        let (mut harness, system) = create_harness(false);
        let cache_a_nonposted_write_request = MemoryAccess::new(
            harness.engine.top(),
            AccessType::WriteNonPostedRequest,
            95,
            DST_ADDR + 1,
            SRC_ADDR,
            BACKING_MEMORY_DEVICE_ID,
            CPU_A_DEVICE_ID,
            OVERHEAD_SIZE_BYTES,
        )
        .with_cache_hint(CacheHintType::NoAllocate);

        // Write the original unaligned range without allocating cache lines.
        harness.run_steps([par!([
            send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_nonposted_write_request),
            expect_dev_tx!(
                CACHE_A_CLIENT_PORT,
                MemoryTxn::write_np_rsp(DST_ADDR + 1).with_bytes(95)
            )
        ])]);

        // Forward exactly 95 bytes while respecting the single fill slot.
        assert_eq!(system.memory.bytes_written(), 95);
        assert_eq!(system.cache_a.inflight_fills_snapshot().peak, 1);
    }

    #[test]
    fn dirty_one_way_eviction_writes_back_before_reuse() {
        let (mut harness, system) = create_harness(false);
        let cache_a_victim_nonposted_write_request =
            build_nonposted_write_request_for_cache_a(harness.engine.top(), DST_ADDR);
        let cache_a_replacement_nonposted_write_request = build_nonposted_write_request_for_cache_a(
            harness.engine.top(),
            DST_ADDR + LINE_SIZE_BYTES as u64,
        );
        let cache_b_read_request = build_read_request_for_cache_b(harness.engine.top(), DST_ADDR);

        harness.run_steps([
            // Dirty the first line in the one-way cache.
            par!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_victim_nonposted_write_request),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, MemoryTxn::write_np_rsp(DST_ADDR))
            ]),
            // Reuse the slot, forcing the dirty line to be written back.
            par!([
                send_dev_rx!(
                    CACHE_A_CLIENT_PORT,
                    cache_a_replacement_nonposted_write_request
                ),
                expect_dev_tx!(
                    CACHE_A_CLIENT_PORT,
                    MemoryTxn::write_np_rsp(DST_ADDR + LINE_SIZE_BYTES as u64)
                )
            ]),
            // Read the evicted line from another cache.
            par!([
                send_dev_rx!(CACHE_B_CLIENT_PORT, cache_b_read_request),
                expect_dev_tx!(CACHE_B_CLIENT_PORT, MemoryTxn::read_rsp(DST_ADDR))
            ]),
        ]);

        // The evicted dirty line reaches backing memory before the remote read.
        assert_eq!(system.memory.bytes_written(), LINE_SIZE_BYTES);
        assert_eq!(system.cache_a.inflight_fills_snapshot().peak, 1);
    }

    #[test]
    fn barrier_after_dirty_eviction_waits_for_backing_memory() {
        for routed in [false, true] {
            let (mut harness, system) = create_harness(routed);
            let mut steps = Vec::new();
            // The second write evicts the first line from the one-way cache.
            for address in [DST_ADDR, DST_ADDR + LINE_SIZE_BYTES as u64] {
                let write =
                    build_nonposted_write_request_for_cache_a(harness.engine.top(), address);
                steps.push(par!([
                    send_dev_rx!(CACHE_A_CLIENT_PORT, write),
                    expect_dev_tx!(CACHE_A_CLIENT_PORT, MemoryTxn::write_np_rsp(address)),
                ]));
            }
            let barrier = build_barrier_request_for_cache_a(harness.engine.top(), DST_ADDR);
            steps.push(par!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, barrier),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, MemoryTxn::barrier_rsp(DST_ADDR)),
            ]));
            let memory = system.memory.clone();
            steps.push(script! {
                ports {}
                async {
                    assert_eq!(memory.bytes_written(), LINE_SIZE_BYTES);
                    Ok(())
                }
            });
            harness.run_steps(steps);
        }
    }

    #[test]
    fn routed_fabric_preserves_dirty_writeback_before_remote_read() {
        let (mut harness, system) = create_harness(true);
        let cache_a_nonposted_write_request =
            build_nonposted_write_request_for_cache_a(harness.engine.top(), DST_ADDR);
        let cache_b_read_request = build_read_request_for_cache_b(harness.engine.top(), DST_ADDR);

        // Write through cache A, then read the dirty line through cache B.
        harness.run_steps([
            par!([
                send_dev_rx!(CACHE_A_CLIENT_PORT, cache_a_nonposted_write_request),
                expect_dev_tx!(CACHE_A_CLIENT_PORT, MemoryTxn::write_np_rsp(DST_ADDR))
            ]),
            par!([
                send_dev_rx!(CACHE_B_CLIENT_PORT, cache_b_read_request),
                expect_dev_tx!(CACHE_B_CLIENT_PORT, MemoryTxn::read_rsp(DST_ADDR))
            ]),
        ]);

        // The routed fabric preserves writeback-before-read ordering.
        assert_eq!(system.memory.bytes_written(), LINE_SIZE_BYTES);
        assert_eq!(system.memory.bytes_read(), LINE_SIZE_BYTES);
        assert_eq!(system.cache_a.inflight_fills_snapshot().peak, 1);
        assert_eq!(
            system.cache_a.traffic_snapshot().coherence_sent[CoherenceOp::Writeback as usize],
            1
        );
        assert_eq!(
            system.cache_a.traffic_snapshot().logical_write_bytes,
            LINE_SIZE_BYTES
        );
    }

    fn create_harness(
        routed: bool,
    ) -> (
        TwoCacheHarness<MemoryAccess>,
        Rc<TwoCacheSystem<MemoryAccess>>,
    ) {
        let mut engine = start_test(file!());
        let system = TwoCacheSystem::new_with_fabric(&mut engine, routed);
        let harness =
            TwoCacheHarness::new(engine, system.clone(), NUM_CLIENT_PORTS, NUM_CLIENT_PORTS);
        (harness, system)
    }
}

fn create_cache_config() -> CacheConfig {
    CacheConfig::new(
        LINE_SIZE_BYTES,
        BW_BYTES_PER_CYCLE,
        CACHE_NUM_SETS,
        CACHE_NUM_WAYS,
        DELAY_TICKS,
    )
    .with_admission_limits(1, 1)
}

fn create_memory_map(device_id: DeviceId, start_address: u64, size_bytes: u64) -> MemoryMap {
    let mut memory_map = MemoryMap::new();
    memory_map
        .insert(start_address, size_bytes, device_id)
        .unwrap();
    memory_map
}

fn build_read_request_for_cache_a(created_by: &Rc<Entity>, address: u64) -> MemoryAccess {
    build_request_for_device(
        created_by,
        CPU_A_DEVICE_ID,
        address,
        AccessType::ReadRequest,
    )
}

fn build_read_request_for_cache_b(created_by: &Rc<Entity>, address: u64) -> MemoryAccess {
    build_request_for_device(
        created_by,
        CPU_B_DEVICE_ID,
        address,
        AccessType::ReadRequest,
    )
}

struct TwoCacheSystem<T>
where
    T: SimObject + Routable + CoherentAccess + AccessMemory,
{
    fabric: Rc<dyn Fabric<T>>,
    manager: Rc<CoherencyManager>,
    cache_a: Rc<Cache<T>>,
    cache_b: Rc<Cache<T>>,
    memory: Rc<Memory<T>>,
}

impl<T> TwoCacheSystem<T>
where
    T: SimObject + Routable + CoherentAccess + AccessMemory,
{
    fn port_dev_rx_i(&self, index: usize) -> PortStateResult<T> {
        self.cache(index).port_dev_rx()
    }

    fn connect_port_dev_tx_i(&self, index: usize, port_state: PortStateResult<T>) -> SimResult {
        self.cache(index).connect_port_dev_tx(port_state)
    }

    fn cache(&self, index: usize) -> &Rc<Cache<T>> {
        match index {
            CACHE_A_CLIENT_PORT => &self.cache_a,
            CACHE_B_CLIENT_PORT => &self.cache_b,
            _ => panic!("invalid cache index {index}"),
        }
    }
}

impl TwoCacheSystem<MemoryAccess> {
    fn new_with_fabric(engine: &mut Engine, routed: bool) -> Rc<Self> {
        let fabric = Self::create_fabric(engine, routed);
        let [cache_a, cache_b] = Self::create_caches(engine);
        let (manager, memory) = Self::create_backing_memory(engine);
        let system = Rc::new(Self {
            fabric,
            manager,
            cache_a,
            cache_b,
            memory,
        });
        system.connect_fabric();
        system
    }

    fn create_fabric(engine: &mut Engine, routed: bool) -> Rc<dyn Fabric<MemoryAccess>> {
        let clock = engine.default_clock();
        let top = engine.top();
        let config = Self::create_fabric_config();
        if routed {
            gwr_models::fabric::routed::RoutedFabric::new_and_register(
                engine,
                &clock,
                top,
                "fabric",
                config,
                gwr_models::fabric::node::FabricRoutingAlgorithm::ColumnFirst,
            )
            .unwrap()
        } else {
            FunctionalFabric::new_and_register(engine, &clock, top, "fabric", config).unwrap()
        }
    }

    fn create_fabric_config() -> Rc<FabricConfig> {
        Rc::new(
            FabricConfig::new(
                FabricGeometry {
                    num_columns: 1,
                    num_rows: NUM_FABRIC_PORTS,
                    num_ports_per_node: 1,
                    ports_per_node_limit: None,
                },
                FabricPortConfig {
                    ticks_per_hop: 1,
                    ticks_overhead: 1,
                    rx_buffer_bytes: FABRIC_BUFFER_SIZE_BYTES,
                    tx_buffer_bytes: FABRIC_BUFFER_SIZE_BYTES,
                    port_bits_per_tick: FABRIC_BANDWIDTH_BITS_PER_TICK,
                },
                [
                    (CACHE_A_DEVICE_ID, CACHE_A_FABRIC_PORT),
                    (CACHE_B_DEVICE_ID, CACHE_B_FABRIC_PORT),
                    (DIRECTORY_DEVICE_ID, DIRECTORY_FABRIC_PORT),
                    (BACKING_MEMORY_DEVICE_ID, BACKING_MEMORY_FABRIC_PORT),
                ]
                .into_iter()
                .map(|(device_id, port)| (device_id.0, vec![port]))
                .collect(),
            )
            .unwrap(),
        )
    }

    fn create_caches(engine: &mut Engine) -> [Rc<Cache<MemoryAccess>>; 2] {
        let clock = engine.default_clock();
        let manager_map = Rc::new(create_memory_map(
            DIRECTORY_DEVICE_ID,
            BASE_ADDRESS,
            BACKING_MEMORY_CAPACITY_BYTES as u64,
        ));
        [
            ("cache_a", CACHE_A_DEVICE_ID),
            ("cache_b", CACHE_B_DEVICE_ID),
        ]
        .map(|(name, device_id)| {
            Cache::new_coherent_and_register(
                engine,
                &clock,
                engine.top(),
                name,
                create_cache_config(),
                CoherenceConfig::new(device_id, manager_map.clone()),
            )
            .unwrap()
        })
    }

    fn create_backing_memory(
        engine: &mut Engine,
    ) -> (Rc<CoherencyManager>, Rc<Memory<MemoryAccess>>) {
        let clock = engine.default_clock();
        let top = engine.top();
        let memory_map = create_memory_map(
            BACKING_MEMORY_DEVICE_ID,
            BASE_ADDRESS,
            BACKING_MEMORY_CAPACITY_BYTES as u64,
        );
        let manager = CoherencyManager::new_and_register(
            engine,
            &clock,
            top,
            "directory",
            CoherencyManagerConfig::new(LINE_SIZE_BYTES, DIRECTORY_DEVICE_ID, memory_map),
        )
        .unwrap();
        let memory = Memory::new_and_register(
            engine,
            &clock,
            top,
            "memory",
            MemoryConfig::new(
                BASE_ADDRESS,
                BACKING_MEMORY_CAPACITY_BYTES,
                BW_BYTES_PER_CYCLE,
                DELAY_TICKS,
            ),
        )
        .unwrap();
        (manager, memory)
    }

    fn connect_fabric(&self) {
        let fabric = &self.fabric;
        for (cache, port) in [
            (&self.cache_a, CACHE_A_FABRIC_PORT),
            (&self.cache_b, CACHE_B_FABRIC_PORT),
        ] {
            connect_port!(cache, mem_tx => fabric, ingress, port).unwrap();
            connect_port!(fabric, egress, port => cache, mem_rx).unwrap();
        }
        let manager = &self.manager;
        connect_port!(manager, tx => fabric, ingress, DIRECTORY_FABRIC_PORT).unwrap();
        connect_port!(fabric, egress, DIRECTORY_FABRIC_PORT => manager, rx).unwrap();
        let memory = &self.memory;
        connect_port!(memory, tx => fabric, ingress, BACKING_MEMORY_FABRIC_PORT).unwrap();
        connect_port!(fabric, egress, BACKING_MEMORY_FABRIC_PORT => memory, rx).unwrap();
    }
}

fn build_write_request_for_cache_a(created_by: &Rc<Entity>, address: u64) -> MemoryAccess {
    build_request_for_device(
        created_by,
        CPU_A_DEVICE_ID,
        address,
        AccessType::WriteRequest,
    )
}

fn build_nonposted_write_request_for_cache_a(
    created_by: &Rc<Entity>,
    address: u64,
) -> MemoryAccess {
    build_request_for_device(
        created_by,
        CPU_A_DEVICE_ID,
        address,
        AccessType::WriteNonPostedRequest,
    )
}

fn build_request_for_device(
    created_by: &Rc<Entity>,
    source_device_id: DeviceId,
    destination_address: u64,
    access_type: AccessType,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        access_type,
        ACCESS_SIZE_BYTES,
        destination_address,
        SRC_ADDR,
        BACKING_MEMORY_DEVICE_ID,
        source_device_id,
        OVERHEAD_SIZE_BYTES,
    )
}

fn build_barrier_request_for_cache_a(created_by: &Rc<Entity>, address: u64) -> MemoryAccess {
    build_barrier_request(created_by, address, CPU_A_DEVICE_ID)
}

fn build_barrier_request(
    created_by: &Rc<Entity>,
    destination_address: u64,
    source_device_id: DeviceId,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::BarrierRequest,
        0,
        destination_address,
        0,
        DIRECTORY_DEVICE_ID,
        source_device_id,
        OVERHEAD_SIZE_BYTES,
    )
}
