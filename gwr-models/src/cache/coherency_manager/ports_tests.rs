// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_engine::traits::Routable;
use gwr_engine::types::AccessType;

use super::*;
use crate::build_model_harness;
use crate::cache::coherency_manager::CoherenceOp;
use crate::cache::traits::CoherentAccess;
use crate::memory::traits::{AccessMemory, CacheAccess, CacheFillToken, ReadMemory};

const DIRECTORY_DEVICE_ID: DeviceId = DeviceId(1);
const CLIENT: DeviceId = DeviceId(2);
const MEMORY: DeviceId = DeviceId(9);
const LINE_BYTES: usize = 32;
const REQUESTS: usize = 7;

build_model_harness! {
    harness ManagerHarness<T: CoherentAccess> {
        component: manager: Rc<CoherencyManager<T>>,
        rx ports: { Rx<T> => rx },
        tx ports: { Tx<T> => tx },
    }
}

#[test]
fn incoming_packets_share_a_budget_across_same_tick_wakeups() {
    for limit in [None, Some(1), Some(2), Some(3)] {
        let (mut harness, _) = create_harness(limit);
        let requests = (0..REQUESTS).map(|i| request(&harness.engine, AccessType::Control, i));
        let sends = requests.map(|packet| send_rx!(packet)).collect::<Vec<_>>();
        let clock = harness.clock.clone();
        harness.run_steps([par!([
            seq!(sends),
            script! {
                ports { output = Tx }
                async {
                    let start = clock.tick_now().tick();
                    for i in 0..REQUESTS {
                        let grant: MemoryAccess = output.get()?.await;
                        assert_eq!(grant.coherence_op(), Some(CoherenceOp::GrantExclusive));
                        assert_eq!(grant.dst_addr(), (i * LINE_BYTES) as u64);
                        let expected = limit.map_or(0, |budget| i / budget) as u64;
                        assert_eq!(clock.tick_now().tick() - start, expected);
                    }
                    Ok(())
                }
            },
        ])]);
    }
}

#[test]
fn barrier_release_replays_one_request_per_processing_slot() {
    let (mut harness, _) = create_harness(Some(1));
    let read = request(&harness.engine, AccessType::ReadRequest, 0);
    let barrier = request(&harness.engine, AccessType::BarrierRequest, 0);
    let requests = (1..=REQUESTS)
        .map(|i| request(&harness.engine, AccessType::Control, i))
        .collect::<Vec<_>>();
    let clock = harness.clock.clone();
    harness.run_steps([script! {
            ports { output = Tx, input = Rx }
            async {
                input.put(read)?.await;
                let memory_read: MemoryAccess = output.get()?.await;
                input.put(barrier)?.await;
                for packet in requests { input.put(packet)?.await; }
                // Let all requests reach the manager behind the barrier.
                clock.wait_ticks((REQUESTS + 3) as u64)?.await;
                input.put(memory_read.to_response(&DummyMemory)?)?.await;
                let grant: MemoryAccess = output.get()?.await;
                assert_eq!(grant.coherence_op(), Some(CoherenceOp::GrantShared));
                let start = clock.tick_now().tick();
                let barrier: MemoryAccess = output.get()?.await;
                assert_eq!(barrier.access_type(), AccessType::BarrierResponse);
                assert_eq!(clock.tick_now().tick(), start + 1);
                for i in 1..=REQUESTS {
                    let grant: MemoryAccess = output.get()?.await;
                    assert_eq!(grant.coherence_op(), Some(CoherenceOp::GrantExclusive));
                    assert_eq!(clock.tick_now().tick(), start + 1 + i as u64);
                }
                Ok(())
            }
    }]);
}

#[test]
fn same_line_queue_replays_one_request_per_processing_slot() {
    let (mut harness, _) = create_harness(Some(1));
    let read = request(&harness.engine, AccessType::ReadRequest, 0);
    let requests = (1..=REQUESTS)
        .map(|i| {
            let packet = request(&harness.engine, AccessType::Control, i);
            packet
                .build_coherence_child_request(
                    AccessType::Control,
                    0,
                    LINE_BYTES,
                    packet.cache_fill_token().unwrap().clone(),
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let clock = harness.clock.clone();
    harness.run_steps([script! {
        ports { output = Tx, input = Rx }
        async {
            input.put(read)?.await;
            let memory_read: MemoryAccess = output.get()?.await;
            for packet in requests { input.put(packet)?.await; }
            clock.wait_ticks((REQUESTS + 3) as u64)?.await;
            input.put(memory_read.to_response(&DummyMemory)?)?.await;
            let grant: MemoryAccess = output.get()?.await;
            assert_eq!(grant.coherence_op(), Some(CoherenceOp::GrantShared));
            let start = clock.tick_now().tick();
            for i in 1..=REQUESTS {
                let grant: MemoryAccess = output.get()?.await;
                assert_eq!(grant.coherence_op(), Some(CoherenceOp::GrantExclusive));
                assert_eq!(clock.tick_now().tick(), start + i as u64);
            }
            Ok(())
        }
    }]);
}

#[test]
fn idle_time_refreshes_the_budget_without_accumulating_credit() {
    let (mut harness, _) = create_harness(Some(2));
    let requests = (0..REQUESTS)
        .map(|i| request(&harness.engine, AccessType::Control, i))
        .collect::<Vec<_>>();
    let clock = harness.clock.clone();
    harness.run_steps([script! {
        ports { output = Tx, input = Rx }
        async {
            let mut requests = requests.into_iter();
            input.put(requests.next().unwrap())?.await;
            let _: MemoryAccess = output.get()?.await;
            clock.wait_ticks(5)?.await;
            let start = clock.tick_now().tick();
            for (i, packet) in requests.enumerate() {
                input.put(packet)?.await;
                let _: MemoryAccess = output.get()?.await;
                assert_eq!(clock.tick_now().tick() - start, (i / 2) as u64);
            }
            Ok(())
        }
    }]);
}

#[test]
fn protocol_traffic_bypasses_request_backlog_and_output_backpressure() {
    let (mut harness, manager) = create_harness(Some(1));
    let owner = request(&harness.engine, AccessType::Control, 0);
    let read = request(&harness.engine, AccessType::ReadRequest, 0)
        .with_routing(DIRECTORY_DEVICE_ID, DeviceId(3));
    let barrier = request(&harness.engine, AccessType::BarrierRequest, 0);
    let requests = (1..=REQUESTS)
        .map(|i| request(&harness.engine, AccessType::Control, i))
        .collect::<Vec<_>>();
    let clock = harness.clock.clone();
    harness.run_steps([script! {
        ports { output = Tx, input = Rx }
        async {
            input.put(owner)?.await;
            let _: MemoryAccess = output.get()?.await;
            input.put(read)?.await;
            let invalidate: MemoryAccess = output.get()?.await;
            assert_eq!(invalidate.coherence_op(), Some(CoherenceOp::Invalidate));
            let start = clock.tick_now().tick();
            input.put(barrier)?.await;
            for packet in requests { input.put(packet)?.await; }
            let ack = invalidate.with_coherence_op(Some(CoherenceOp::InvalidateAck))
                .with_routing(DIRECTORY_DEVICE_ID, CLIENT);
            input.put(ack)?.await;
            let memory_read: MemoryAccess = output.get()?.await;
            assert_eq!(memory_read.access_type(), AccessType::ReadRequest);
            assert_eq!(clock.tick_now().tick(), start + 1);
            input.put(memory_read.to_response(&DummyMemory)?)?.await;

            // Stall the output while the processing worker completes the read,
            // barrier, and later acquisitions using the shared budget.
            clock.wait_ticks((REQUESTS + 4) as u64)?.await;
            assert_eq!(manager.received_coherence_op_count(CoherenceOp::ExclusiveWrite), REQUESTS + 1);
            assert_eq!(manager.sent_coherence_op_count(CoherenceOp::GrantExclusive), 1);
            assert_eq!(manager.sent_coherence_op_count(CoherenceOp::GrantShared), 0);
            let grant: MemoryAccess = output.get()?.await;
            assert_eq!(grant.coherence_op(), Some(CoherenceOp::GrantShared));
            let barrier: MemoryAccess = output.get()?.await;
            assert_eq!(barrier.access_type(), AccessType::BarrierResponse);
            for _ in 0..REQUESTS {
                let grant: MemoryAccess = output.get()?.await;
                assert_eq!(grant.coherence_op(), Some(CoherenceOp::GrantExclusive));
            }
            // Let the final sender handshake finish before observing its count.
            clock.wait_ticks(1)?.await;
            assert_eq!(manager.sent_coherence_op_count(CoherenceOp::GrantExclusive), REQUESTS + 1);
            assert_eq!(manager.sent_coherence_op_count(CoherenceOp::GrantShared), 1);
            assert_eq!(manager.sent_coherence_op_count(CoherenceOp::Invalidate), 1);
            assert_eq!(manager.sent_coherence_message_count(), REQUESTS + 3);
            Ok(())
        }
    }]);
}

#[test]
fn consecutive_barriers_charge_for_both_start_and_completion() {
    for limit in [None, Some(1), Some(2), Some(3)] {
        let (mut harness, _) = create_harness(limit);
        let sends = (0..REQUESTS)
            .map(|i| send_rx!(request(&harness.engine, AccessType::BarrierRequest, i)))
            .collect::<Vec<_>>();
        let clock = harness.clock.clone();
        harness.run_steps([par!([
            seq!(sends),
            script! {
                ports { output = Tx }
                async {
                    let start = clock.tick_now().tick();
                    for i in 0..REQUESTS {
                        let response: MemoryAccess = output.get()?.await;
                        assert_eq!(response.access_type(), AccessType::BarrierResponse);
                        assert_eq!(response.dst_addr(), (i * LINE_BYTES) as u64);
                        let elapsed = limit.map_or(0, |budget| (2 * i + 1) / budget) as u64;
                        assert_eq!(clock.tick_now().tick() - start, elapsed);
                    }
                    Ok(())
                }
            },
        ])]);
    }
}

#[test]
fn writeback_preempts_a_ready_line_and_its_acknowledgement_resumes_the_queue() {
    let (mut harness, _) = create_harness(Some(1));
    let read = request(&harness.engine, AccessType::ReadRequest, 0);
    let acquisition = request(&harness.engine, AccessType::Control, 0);
    let writeback = request(&harness.engine, AccessType::WriteRequest, 0)
        .with_coherence_op(Some(CoherenceOp::Writeback));
    let clock = harness.clock.clone();
    harness.run_steps([script! {
        ports { output = Tx, input = Rx }
        async {
            input.put(read)?.await;
            let memory_read: MemoryAccess = output.get()?.await;
            input.put(acquisition.clone())?.await;
            input.put(acquisition)?.await;
            clock.wait_ticks(4)?.await;
            input.put(memory_read.to_response(&DummyMemory)?)?.await;
            let shared: MemoryAccess = output.get()?.await;
            assert_eq!(shared.coherence_op(), Some(CoherenceOp::GrantShared));
            let first_grant: MemoryAccess = output.get()?.await;
            assert_eq!(first_grant.coherence_op(), Some(CoherenceOp::GrantExclusive));
            let start = clock.tick_now().tick();
            input.put(writeback)?.await;
            let memory_write: MemoryAccess = output.get()?.await;
            assert_eq!(memory_write.access_type(), AccessType::WriteNonPostedRequest);
            assert_eq!(clock.tick_now().tick(), start + 1);
            input.put(memory_write.to_ack()?)?.await;
            let second_grant: MemoryAccess = output.get()?.await;
            assert_eq!(second_grant.coherence_op(), Some(CoherenceOp::GrantExclusive));
            assert_eq!(clock.tick_now().tick(), start + 3);
            Ok(())
        }
    }]);
}

#[test]
fn zero_processing_budget_is_rejected() {
    assert!(
        CoherencyManagerConfig::new(LINE_BYTES, DIRECTORY_DEVICE_ID, MemoryMap::new())
            .with_operations_per_tick(0)
            .validate()
            .is_err()
    );
}

fn create_harness(limit: Option<usize>) -> (ManagerHarness<MemoryAccess>, Rc<CoherencyManager>) {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let mut map = MemoryMap::new();
    map.insert(0, 4096, MEMORY).unwrap();
    let mut config = CoherencyManagerConfig::new(LINE_BYTES, DIRECTORY_DEVICE_ID, map);
    if let Some(limit) = limit {
        config = config.with_operations_per_tick(limit);
    }
    let manager =
        CoherencyManager::new_and_register(&engine, &clock, engine.top(), "directory", config)
            .unwrap();
    (ManagerHarness::new(engine, manager.clone()), manager)
}

fn request(engine: &Engine, kind: AccessType, ordinal: usize) -> MemoryAccess {
    MemoryAccess::new(
        engine.top(),
        kind,
        LINE_BYTES,
        (ordinal * LINE_BYTES) as u64,
        0,
        DIRECTORY_DEVICE_ID,
        CLIENT,
        16,
    )
    .build_coherence_child_request(
        kind,
        (ordinal * LINE_BYTES) as u64,
        LINE_BYTES,
        CacheFillToken {
            owner: Rc::new(()),
            ordinal: ordinal as u64,
        },
    )
    .unwrap()
    .with_coherence_op(match kind {
        AccessType::Control => Some(CoherenceOp::ExclusiveWrite),
        AccessType::ReadRequest => Some(CoherenceOp::SharedRead),
        _ => None,
    })
}

struct DummyMemory;
impl ReadMemory for DummyMemory {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}
