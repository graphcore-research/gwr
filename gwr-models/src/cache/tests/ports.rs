// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Exercise the whole cache through its device and memory ports. The scripted
//! memory peer controls response ordering without driving cache internals.
use std::rc::Rc;

use gwr_components::test_helpers::{expect_no_traffic, expect_value};
use gwr_engine::engine::Engine;
use gwr_engine::traits::TotalBytes;
use gwr_engine::types::{AccessType, DeviceId};
use gwr_track::entity::{Entity, GetEntity};

use crate::build_model_harness;
use crate::cache::coherency_manager::CoherenceOp;
use crate::cache::traits::{CoherenceToken, CoherentAccess};
use crate::cache::{Cache, CacheConfig, CoherenceConfig};
use crate::memory::CacheHintType;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::memory::traits::{AccessMemory, CacheAccess, ReadMemory};
use crate::test_helpers::MemoryTxn;

const CACHE_DEVICE_ID: DeviceId = DeviceId(1);
const DIRECTORY_DEVICE_ID: DeviceId = DeviceId(3);
const BACKING_MEMORY_DEVICE_ID: DeviceId = DeviceId(4);
const CLIENT_DEVICE_ID: DeviceId = DeviceId(5);
const LINE_SIZE_BYTES: usize = 32;
const BANDWIDTH_BYTES_PER_TICK: usize = 8;
const HEADER_SIZE_BYTES: usize = 16;
// Longer than ingress, request, and return service for a single-line packet.
const QUIET_TICKS: u64 =
    4 * (LINE_SIZE_BYTES + HEADER_SIZE_BYTES).div_ceil(BANDWIDTH_BYTES_PER_TICK) as u64;

build_model_harness! {
    harness CacheHarness<T: CacheAccess> {
        component: cache: Rc<Cache<T>>,
        rx ports: { DevRx<T> => dev_rx, MemRx<T> => mem_rx },
        tx ports: { DevTx<T> => dev_tx, MemTx<T> => mem_tx },
    }
}

#[test]
fn coalesced_reads_release_an_invalidated_fill_slot_for_a_waiting_read() {
    let (mut harness, cache) = create_harness(false, 4, 1);
    let read = request(&cache, AccessType::ReadRequest, 0, 1);
    let invalidate = request(&cache, AccessType::Control, 0, 1);
    let noallocate = read.clone().with_cache_hint(CacheHintType::NoAllocate);
    let clock = harness.clock.clone();

    harness.run_steps([
        par!([
            seq!([
                send_dev_rx!(read.clone()),
                send_dev_rx!(read),
                send_dev_rx!(invalidate),
                send_dev_rx!(noallocate),
            ]),
            script! {
                ports { requests = MemTx, responses = MemRx, client = DevTx }
                async {
                    let fill: MemoryAccess = expect_value(
                        &mut requests,
                        &MemoryTxn::read_req(0).with_bytes(LINE_SIZE_BYTES),
                        "original fill",
                    ).await?;
                    expect_no_traffic(
                        "invalidated fill still occupies the only slot", &clock, QUIET_TICKS,
                        vec![("memory", &requests), ("client", &client)],
                    ).await?;
                    responses.put(fill.to_response(&DummyMemory)?)?.await;
                    for _ in 0..2 {
                        expect_value(&mut client, &MemoryTxn::read_rsp(0).with_bytes(1), "coalesced read").await?;
                    }
                    let replacement: MemoryAccess = expect_value(
                        &mut requests,
                        &MemoryTxn::read_req(0).with_bytes(LINE_SIZE_BYTES),
                        "waiting noallocate fill",
                    ).await?;
                    assert_eq!(replacement.cache_hint(), CacheHintType::NoAllocate);
                    responses.put(replacement.to_response(&DummyMemory)?)?.await;
                    expect_value(&mut client, &MemoryTxn::read_rsp(0).with_bytes(1), "noallocate read").await?;
                    Ok(())
                }
            },
        ]),
        expect_no_traffic!(&[Port::MemTx, Port::DevTx], QUIET_TICKS),
    ]);

    assert_eq!(cache.traffic_snapshot().coalesced_lines, 1);
    assert_eq!(cache.traffic_snapshot().fill_requests, 2);
    assert_eq!(cache.inflight_fills_snapshot().current, 0);
    assert_eq!(cache.inflight_fills_snapshot().peak, 1);
}

#[test]
fn coherent_multiline_nonposted_write_completes_once_with_header_only_acquisitions() {
    let (mut harness, cache) = create_harness(true, 1, 1);
    let write = request(
        &cache,
        AccessType::WriteNonPostedRequest,
        1,
        2 * LINE_SIZE_BYTES - 1,
    );
    let clock = harness.clock.clone();
    harness.run_steps([
        par!([
            send_dev_rx!(write),
            script! {
                ports { requests = MemTx, responses = MemRx, client = DevTx }
                async {
                    for address in [0, LINE_SIZE_BYTES as u64] {
                        let acquisition: MemoryAccess = expect_value(
                            &mut requests,
                            &MemoryTxn::new(AccessType::Control, address)
                                .with_bytes(LINE_SIZE_BYTES).with_total_bytes(HEADER_SIZE_BYTES),
                            "exclusive acquisition",
                        ).await?;
                        assert_eq!(acquisition.coherence_op(), Some(CoherenceOp::ExclusiveWrite));
                        expect_no_traffic(
                            "write waits for each exclusive grant", &clock, QUIET_TICKS,
                            vec![("memory", &requests), ("client", &client)],
                        ).await?;
                        let grant = acquisition
                            .to_ack()?
                            .with_coherence_op(Some(CoherenceOp::GrantExclusive));
                        responses.put(grant)?.await;
                    }
                    expect_value(
                        &mut client,
                        &MemoryTxn::write_np_rsp(1).with_bytes(2 * LINE_SIZE_BYTES - 1)
                            .with_total_bytes(HEADER_SIZE_BYTES),
                        "original write acknowledgement",
                    ).await?;
                    Ok(())
                }
            },
        ]),
        expect_no_traffic!(&[Port::DevTx, Port::MemTx], QUIET_TICKS),
    ]);
    assert_eq!(cache.inflight_fills_snapshot().current, 0);
    assert_eq!(cache.inflight_fills_snapshot().peak, 1);
    assert_eq!(
        cache.traffic_snapshot().coherence_sent[CoherenceOp::ExclusiveWrite as usize],
        2
    );
}

#[test]
fn coherent_dirty_victim_is_written_back_before_replacement_fill() {
    let (mut harness, cache) = create_harness(true, 1, 1);
    let write = request(&cache, AccessType::WriteNonPostedRequest, 0, 1);
    let read = request(
        &cache,
        AccessType::ReadRequest,
        2 * LINE_SIZE_BYTES as u64,
        1,
    );
    harness.run_steps([
        par!([
            seq!([
                send_dev_rx!(write),
                expect_dev_tx!(MemoryTxn::write_np_rsp(0).with_bytes(1)),
                send_dev_rx!(read),
                expect_dev_tx!(MemoryTxn::read_rsp(2 * LINE_SIZE_BYTES as u64).with_bytes(1)),
            ]),
            script! {
                ports { requests = MemTx, responses = MemRx }
                async {
                    let acquisition: MemoryAccess = requests.get()?.await;
                    assert_eq!(acquisition.coherence_op(), Some(CoherenceOp::ExclusiveWrite));
                    let grant = acquisition
                        .to_ack()?
                        .with_coherence_op(Some(CoherenceOp::GrantExclusive));
                    responses.put(grant)?.await;
                    let writeback: MemoryAccess = expect_value(
                        &mut requests,
                        &MemoryTxn::write_req(0).with_bytes(LINE_SIZE_BYTES), "dirty victim",
                    ).await?;
                    assert_eq!(writeback.coherence_op(), Some(CoherenceOp::Writeback));
                    let fill: MemoryAccess = expect_value(
                        &mut requests,
                        &MemoryTxn::read_req(2 * LINE_SIZE_BYTES as u64).with_bytes(LINE_SIZE_BYTES),
                        "replacement fill after writeback",
                    ).await?;
                    assert_eq!(fill.coherence_op(), Some(CoherenceOp::SharedRead));
                    let grant = fill
                        .to_response(&DummyMemory)?
                        .with_coherence_op(Some(CoherenceOp::GrantShared));
                    responses.put(grant)?.await;
                    Ok(())
                }
            },
        ]),
        expect_no_traffic!(&[Port::DevTx, Port::MemTx], QUIET_TICKS),
    ]);
    assert_eq!(
        cache.traffic_snapshot().downstream_write_bytes,
        LINE_SIZE_BYTES
    );
}

#[test]
fn coherent_late_grant_cannot_reinstall_invalidated_slot() {
    let (mut harness, cache) = create_harness(true, 1, 1);
    let read = request(&cache, AccessType::ReadRequest, 0, 1);
    harness.run_steps([
        par!([
            seq!([
                send_dev_rx!(read.clone()),
                expect_dev_tx!(MemoryTxn::read_rsp(0).with_bytes(1)),
                send_dev_rx!(read),
                expect_dev_tx!(MemoryTxn::read_rsp(0).with_bytes(1)),
            ]),
            script! {
                ports { requests = MemTx, responses = MemRx }
                async {
                    let fill: MemoryAccess = requests.get()?.await;
                    let invalidate = invalidation(&fill);
                    responses.put(invalidate.clone())?.await;
                    let ack: MemoryAccess = requests.get()?.await;
                    assert_eq!(ack.coherence_op(), Some(CoherenceOp::InvalidateAck));
                    assert!(ack.coherence_token().unwrap().matches(invalidate.coherence_token().unwrap()));
                    let grant = fill
                        .to_response(&DummyMemory)?
                        .with_coherence_op(Some(CoherenceOp::GrantShared));
                    responses.put(grant)?.await;
                    // The next read must fetch again despite the late grant.
                    let refill: MemoryAccess = expect_value(
                        &mut requests, &MemoryTxn::read_req(0).with_bytes(LINE_SIZE_BYTES), "refill",
                    ).await?;
                    let grant = refill
                        .to_response(&DummyMemory)?
                        .with_coherence_op(Some(CoherenceOp::GrantShared));
                    responses.put(grant)?.await;
                    Ok(())
                }
            },
        ]),
        expect_no_traffic!(&[Port::DevTx, Port::MemTx], QUIET_TICKS),
    ]);
    assert_eq!(cache.num_misses(), 2);
    assert_eq!(cache.inflight_fills_snapshot().current, 0);
}

#[test]
fn coherent_writer_waits_for_shared_fill_then_acquires_exclusive_permission() {
    let (mut harness, cache) = create_harness(true, 2, 2);
    let read = request(&cache, AccessType::ReadRequest, 0, 1);
    let write = request(&cache, AccessType::WriteNonPostedRequest, 0, 1);
    let clock = harness.clock.clone();
    harness.run_steps([
        par!([
            seq!([send_dev_rx!(read), send_dev_rx!(write)]),
            script! {
                ports { requests = MemTx, responses = MemRx, client = DevTx }
                async {
                    let fill: MemoryAccess = requests.get()?.await;
                    assert_eq!(fill.coherence_op(), Some(CoherenceOp::SharedRead));
                    expect_no_traffic(
                        "writer waits for the shared fill", &clock, QUIET_TICKS,
                        vec![("memory", &requests), ("client", &client)],
                    ).await?;
                    let grant = fill
                        .to_response(&DummyMemory)?
                        .with_coherence_op(Some(CoherenceOp::GrantShared));
                    responses.put(grant)?.await;
                    expect_value(&mut client, &MemoryTxn::read_rsp(0).with_bytes(1), "shared reader").await?;
                    let acquisition: MemoryAccess = requests.get()?.await;
                    assert_eq!(acquisition.coherence_op(), Some(CoherenceOp::ExclusiveWrite));
                    assert_eq!(acquisition.total_bytes(), HEADER_SIZE_BYTES);
                    expect_no_traffic(
                        "shared permission cannot complete the writer", &clock, QUIET_TICKS,
                        vec![("client", &client)],
                    ).await?;
                    let grant = acquisition
                        .to_ack()?
                        .with_coherence_op(Some(CoherenceOp::GrantExclusive));
                    responses.put(grant)?.await;
                    expect_value(&mut client, &MemoryTxn::write_np_rsp(0).with_bytes(1), "exclusive writer").await?;
                    Ok(())
                }
            },
        ]),
        expect_no_traffic!(&[Port::DevTx, Port::MemTx], QUIET_TICKS),
    ]);
}

#[test]
fn invalidation_before_exclusive_grant_waits_for_write_then_writes_back_before_ack() {
    let (mut harness, cache) = create_harness(true, 1, 1);
    let write = request(&cache, AccessType::WriteNonPostedRequest, 0, 1);
    let clock = harness.clock.clone();
    harness.run_steps([
        par!([
            send_dev_rx!(write),
            script! {
                ports { requests = MemTx, responses = MemRx, client = DevTx }
                async {
                    let acquisition: MemoryAccess = requests.get()?.await;
                    let invalidate = invalidation(&acquisition);
                    responses.put(invalidate.clone())?.await;
                    expect_no_traffic(
                        "invalidation waits for exclusive grant", &clock, QUIET_TICKS,
                        vec![("memory", &requests), ("client", &client)],
                    ).await?;
                    let grant = acquisition
                        .to_ack()?
                        .with_coherence_op(Some(CoherenceOp::GrantExclusive));
                    responses.put(grant)?.await;
                    let writeback: MemoryAccess = expect_value(
                        &mut requests, &MemoryTxn::write_req(0).with_bytes(LINE_SIZE_BYTES), "writeback before ack",
                    ).await?;
                    assert_eq!(writeback.coherence_op(), Some(CoherenceOp::Writeback));
                    let ack: MemoryAccess = requests.get()?.await;
                    assert_eq!(ack.coherence_op(), Some(CoherenceOp::InvalidateAck));
                    assert!(ack.coherence_token().unwrap().matches(invalidate.coherence_token().unwrap()));
                    expect_value(&mut client, &MemoryTxn::write_np_rsp(0).with_bytes(1), "completed write").await?;
                    Ok(())
                }
            },
        ]),
        expect_no_traffic!(&[Port::DevTx, Port::MemTx], QUIET_TICKS),
    ]);
    assert_eq!(cache.inflight_fills_snapshot().current, 0);
}

#[test]
fn coherent_final_address_line_is_checked_without_wrapping() {
    let (mut harness, cache) = create_harness(true, 1, 1);
    let read = request(&cache, AccessType::ReadRequest, u64::MAX, 1);
    harness.run_steps([par!([
        seq!([
            send_dev_rx!(read),
            expect_dev_tx!(MemoryTxn::read_rsp(u64::MAX).with_bytes(1)),
        ]),
        script! {
            ports { requests = MemTx, responses = MemRx }
            async {
                let fill: MemoryAccess = expect_value(
                    &mut requests,
                    &MemoryTxn::read_req(u64::MAX - LINE_SIZE_BYTES as u64 + 1).with_bytes(LINE_SIZE_BYTES),
                    "final physical line",
                ).await?;
                let grant = fill
                    .to_response(&DummyMemory)?
                    .with_coherence_op(Some(CoherenceOp::GrantShared));
                responses.put(grant)?.await;
                Ok(())
            }
        },
    ])]);
    assert_eq!(cache.traffic_snapshot().fill_requests, 1);
    assert_eq!(cache.inflight_fills_snapshot().current, 0);
}

#[test]
fn stalled_client_does_not_block_fill_ingress_in_either_mode() {
    for coherent in [false, true] {
        // Allow two independent misses in each coherent or incoherent cache.
        let (mut harness, cache) = create_harness(coherent, 2, 2);
        let first_read = request(&cache, AccessType::ReadRequest, 0, LINE_SIZE_BYTES);
        let second_read = request(
            &cache,
            AccessType::ReadRequest,
            LINE_SIZE_BYTES as u64,
            LINE_SIZE_BYTES,
        );
        let observed_cache = cache.clone();
        let clock = harness.clock.clone();

        harness.run_steps([
            par!([
                seq!([send_dev_rx!(first_read), send_dev_rx!(second_read)]),
                script! {
                    ports { requests = MemTx, responses = MemRx, client = DevTx }
                    async {
                        // Capture both fills without accepting any device responses.
                        let first_fill: MemoryAccess = requests.get()?.await;
                        let second_fill: MemoryAccess = requests.get()?.await;

                        let first_response = first_fill.to_response(&DummyMemory)?
                            .with_coherence_op(coherent.then_some(CoherenceOp::GrantShared));
                        let second_response = second_fill.to_response(&DummyMemory)?
                            .with_coherence_op(coherent.then_some(CoherenceOp::GrantShared));

                        // The second handoff waits only for the first response's
                        // ingress service, independent of its blocked return.
                        let start = clock.tick_now().tick();
                        responses.put(first_response)?.await;
                        responses.put(second_response)?.await;
                        let ingress_ticks = (LINE_SIZE_BYTES + HEADER_SIZE_BYTES)
                            .div_ceil(BANDWIDTH_BYTES_PER_TICK) as u64;
                        assert_eq!(clock.tick_now().tick() - start, ingress_ticks);

                        // Keep the client blocked while both fills finish ingress.
                        clock.wait_ticks(QUIET_TICKS)?.await;
                        assert_eq!(observed_cache.inflight_fills_snapshot().current, 0);

                        // Accept the queued parent responses in request order.
                        for address in [0, LINE_SIZE_BYTES as u64] {
                            expect_value(
                                &mut client,
                                &MemoryTxn::read_rsp(address).with_bytes(LINE_SIZE_BYTES),
                                "queued read",
                            ).await?;
                        }

                        Ok(())
                    }
                },
            ]),
            expect_no_traffic!(&[Port::DevTx, Port::MemTx], QUIET_TICKS),
        ]);
    }
}

#[test]
fn ready_parent_retains_admission_through_return_service_in_both_modes() {
    for coherent in [false, true] {
        // A single parent slot forces the second read to wait for the first.
        let (mut harness, cache) = create_harness(coherent, 1, 1);
        let first_read = request(&cache, AccessType::ReadRequest, 0, LINE_SIZE_BYTES);
        let second_read = request(&cache, AccessType::ReadRequest, LINE_SIZE_BYTES as u64, 1);
        let clock = harness.clock.clone();

        harness.run_steps([
            par!([
                seq!([send_dev_rx!(first_read), send_dev_rx!(second_read)]),
                script! {
                    ports { requests = MemTx, responses = MemRx, client = DevTx }
                    async {
                        // Let the second request reach the occupied admission gate
                        // before responding to the first request's fill.
                        let first_fill: MemoryAccess = requests.get()?.await;
                        clock.wait_ticks(QUIET_TICKS)?.await;

                        let response = first_fill.to_response(&DummyMemory)?
                            .with_coherence_op(coherent.then_some(CoherenceOp::GrantShared));
                        responses.put(response)?.await;

                        // Ingress and return each take response_ticks. Until both
                        // finish, neither the first response nor a new fill is ready.
                        let response_ticks = (LINE_SIZE_BYTES + HEADER_SIZE_BYTES)
                            .div_ceil(BANDWIDTH_BYTES_PER_TICK) as u64;
                        expect_no_traffic(
                            "ready parents retain admission during return service",
                            &clock,
                            2 * response_ticks - 1,
                            vec![("memory", &requests), ("client", &client)],
                        ).await?;

                        // Returning the first parent releases admission for the second.
                        expect_value(
                            &mut client,
                            &MemoryTxn::read_rsp(0).with_bytes(LINE_SIZE_BYTES),
                            "first read",
                        ).await?;
                        let second_fill: MemoryAccess = expect_value(
                            &mut requests,
                            &MemoryTxn::read_req(LINE_SIZE_BYTES as u64).with_bytes(LINE_SIZE_BYTES),
                            "second read after parent return",
                        ).await?;

                        // Complete the second fill and return its one requested byte.
                        let response = second_fill.to_response(&DummyMemory)?
                            .with_coherence_op(coherent.then_some(CoherenceOp::GrantShared));
                        responses.put(response)?.await;
                        expect_value(
                            &mut client,
                            &MemoryTxn::read_rsp(LINE_SIZE_BYTES as u64).with_bytes(1),
                            "second read",
                        ).await?;

                        Ok(())
                    }
                },
            ]),
            expect_no_traffic!(&[Port::DevTx, Port::MemTx], QUIET_TICKS),
        ]);

        assert_eq!(cache.inflight_fills_snapshot().current, 0);
    }
}

#[test]
fn coherent_parent_keeps_admission_until_return_service_completes() {
    let (mut harness, cache) = create_harness(true, 1, 1);
    let read = request(&cache, AccessType::ReadRequest, 0, LINE_SIZE_BYTES);
    let write = request(
        &cache,
        AccessType::WriteNonPostedRequest,
        LINE_SIZE_BYTES as u64,
        1,
    );
    let clock = harness.clock.clone();
    harness.run_steps([
        par!([
            seq!([send_dev_rx!(read), send_dev_rx!(write)]),
            script! {
                ports { requests = MemTx, responses = MemRx, client = DevTx }
                async {
                    let fill: MemoryAccess = requests.get()?.await;
                    // Allow the second request to reach the occupied admission gate.
                    clock.wait_ticks(QUIET_TICKS)?.await;
                    let grant = fill
                        .to_response(&DummyMemory)?
                        .with_coherence_op(Some(CoherenceOp::GrantShared));
                    responses.put(grant)?.await;
                    let response_ticks = (LINE_SIZE_BYTES + HEADER_SIZE_BYTES)
                        .div_ceil(BANDWIDTH_BYTES_PER_TICK) as u64;
                    expect_no_traffic(
                        "admission covers both ingress and response service", &clock, 2 * response_ticks - 1,
                        vec![("memory", &requests), ("client", &client)],
                    ).await?;
                    expect_value(
                        &mut client,
                        &MemoryTxn::read_rsp(0).with_bytes(LINE_SIZE_BYTES),
                        "first parent",
                    ).await?;
                    let acquisition: MemoryAccess = requests.get()?.await;
                    assert_eq!(acquisition.coherence_op(), Some(CoherenceOp::ExclusiveWrite));
                    let grant = acquisition
                        .to_ack()?
                        .with_coherence_op(Some(CoherenceOp::GrantExclusive));
                    responses.put(grant)?.await;
                    expect_value(
                        &mut client, &MemoryTxn::write_np_rsp(LINE_SIZE_BYTES as u64).with_bytes(1), "next parent",
                    ).await?;
                    Ok(())
                }
            },
        ]),
        expect_no_traffic!(&[Port::DevTx, Port::MemTx], QUIET_TICKS),
    ]);
}

#[test]
#[should_panic(expected = "cluster::l1: zero-size cache read/write")]
fn admission_errors_identify_the_cache_in_the_entity_hierarchy() {
    let (mut harness, cache) = create_harness(false, 1, 1);
    let read = request(&cache, AccessType::ReadRequest, 0, 0);
    harness.run_steps([
        send_dev_rx!(read),
        expect_dev_tx!(MemoryTxn::read_rsp(0).with_bytes(0)),
    ]);
}

#[test]
#[should_panic(expected = "cluster::l1: malformed coherence invalidation")]
fn protocol_errors_retain_the_cache_identity_and_original_message() {
    let (mut harness, cache) = create_harness(true, 1, 1);
    let invalidation = request(&cache, AccessType::Control, 0, LINE_SIZE_BYTES)
        .with_coherence_op(Some(CoherenceOp::Invalidate));
    harness.run_steps([
        send_mem_rx!(invalidation),
        expect_mem_tx!(MemoryTxn::new(AccessType::Control, 0)),
    ]);
}

fn create_harness(
    coherent: bool,
    max_parents: usize,
    max_fills: usize,
) -> (CacheHarness<MemoryAccess>, Rc<Cache<MemoryAccess>>) {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let parent = Rc::new(Entity::new(engine.top(), "cluster"));
    let config = CacheConfig::new(LINE_SIZE_BYTES, BANDWIDTH_BYTES_PER_TICK, 2, 1, 0)
        .with_admission_limits(max_parents, max_fills);
    let cache = if coherent {
        let mut map = MemoryMap::new();
        let last_line = u64::MAX - LINE_SIZE_BYTES as u64 + 1;
        map.insert(0, last_line, DIRECTORY_DEVICE_ID).unwrap();
        map.insert(last_line, LINE_SIZE_BYTES as u64, DIRECTORY_DEVICE_ID)
            .unwrap();
        Cache::new_coherent_and_register(
            &engine,
            &clock,
            &parent,
            "l1",
            config,
            CoherenceConfig::new(CACHE_DEVICE_ID, Rc::new(map)),
        )
        .unwrap()
    } else {
        Cache::new_and_register(&engine, &clock, &parent, "l1", config).unwrap()
    };
    (CacheHarness::new(engine, cache.clone()), cache)
}

fn request(
    cache: &Cache<MemoryAccess>,
    kind: AccessType,
    address: u64,
    bytes: usize,
) -> MemoryAccess {
    MemoryAccess::new(
        cache.entity(),
        kind,
        bytes,
        address,
        0,
        BACKING_MEMORY_DEVICE_ID,
        CLIENT_DEVICE_ID,
        HEADER_SIZE_BYTES,
    )
}

fn invalidation(packet: &MemoryAccess) -> MemoryAccess {
    packet
        .clone()
        .with_access_type(AccessType::Control)
        .with_routing(CACHE_DEVICE_ID, DIRECTORY_DEVICE_ID)
        .with_coherence_token(Some(CoherenceToken {
            owner: Rc::new(()),
            ordinal: 0,
        }))
        .with_coherence_op(Some(CoherenceOp::Invalidate))
}

struct DummyMemory;

impl ReadMemory for DummyMemory {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}
