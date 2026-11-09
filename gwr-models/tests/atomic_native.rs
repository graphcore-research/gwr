// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Real direct-port foundation controls. All rates are caller hypotheses.
use std::cell::RefCell;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::{Event, TotalBytes};
use gwr_models::memory::atomic::*;
fn cfg() -> AtomicServiceConfig {
    AtomicServiceConfig {
        capacity: 2,
        bytes_per_tick: 16,
        delay_ticks: 0,
        bf16_pair_per_tick: Some(2),
        i32_fetch_add_per_tick: Some(1),
        lane_interleave_bytes: 4,
        lane_bytes_per_tick: vec![8, 8],
    }
}
#[test]
fn external_actor_handle_only_observes_registers_and_cancels_without_manual_milestones() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let memory = AtomicMemory::new_and_register(
        &engine,
        &clock,
        7,
        0x1000,
        256,
        cfg(),
        AtomicPath::CacheFreeDirect,
    )
    .unwrap();
    let handle: AtomicControlHandle = memory.control_handle();
    let observer = handle.clone();
    handle
        .register_control(ControlGroup {
            invocation: 9,
            generation: 1,
            device: 7,
            signal: 0x1080,
            semaphore: 0x1084,
            participants: 2,
            publisher: 0,
            output_words: vec![0x1000],
            source_flags: SourceFlags::SplitKHgemmSc0Sc1,
        })
        .unwrap();
    assert!(!observer.control_finished(9, 1).unwrap());
    let before = observer.snapshot();
    assert!(handle.cancel(1).is_err());
    assert_eq!(observer.snapshot(), before);
    assert_eq!((before.current, memory.ready_response_count()), (0, 0));
    // The six compile-fail API controls on AtomicControlHandle/AtomicMemory
    // reject raw-service access and intake/start/commit/publication/ACK
    // forgery.
}
fn pair(id: u64, address: u64) -> AtomicPacket {
    AtomicPacket(Request {
        identity: id,
        device: 7,
        address,
        effect: Effect::Atomic {
            participant: id as u32,
            word: AtomicWord {
                device: 7,
                address,
                invocation: 9,
                generation: 1,
                operation: AtomicOperation::Bf16PairFadd,
                scope: AtomicScope::Agent,
                ordering: AtomicOrdering::Monotonic,
            },
        },
    })
}
#[test]
fn actual_stalled_ack_holds_credit_but_does_not_block_same_word_commit() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let memory = AtomicMemory::new_and_register(
        &engine,
        &clock,
        7,
        0x1000,
        256,
        cfg(),
        AtomicPath::CacheFreeDirect,
    )
    .unwrap();
    let mut tx = OutPort::new(engine.top(), "producer");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx = InPort::<AtomicReply>::new(&engine, &clock, engine.top(), "consumer");
    memory.connect_port_tx(rx.state()).unwrap();
    let accepted = Rc::new(RefCell::new(Vec::new()));
    let a = accepted.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        for id in 1..=3 {
            let p = pair(id, 0x1000);
            assert_eq!(p.total_bytes(), 4);
            tx.put(p)?.await;
            a.borrow_mut().push(c.tick_now().tick());
        }
        Ok(())
    });
    let c = clock.clone();
    let s = memory.control_handle();
    let observation = Rc::new(RefCell::new(None));
    let o = observation.clone();
    engine.spawner().spawn(async move {
        c.wait_ticks(10)?.await;
        *o.borrow_mut() = Some(s.snapshot());
        Ok(())
    });
    let c = clock.clone();
    engine.spawner().spawn(async move {
        c.wait_ticks(20)?.await;
        for _ in 0..3 {
            let reply = rx.get()?.await;
            assert_eq!(reply.total_bytes(), 0);
        }
        Ok(())
    });
    engine.run().unwrap();
    let probe = observation.borrow().unwrap();
    assert_eq!(
        (
            probe.committed_requests,
            probe.committed_unacknowledged,
            probe.current
        ),
        (2, 2, 2)
    );
    assert_eq!(accepted.borrow()[0..2], [0, 0]);
    assert!(accepted.borrow()[2] >= 20);
    let s = memory.control_handle().snapshot();
    assert_eq!(
        (
            s.current,
            s.completed,
            s.committed_read_bytes,
            s.committed_write_bytes
        ),
        (0, 3, 12, 12)
    );
}
#[test]
fn actual_granted_publication_cancel_retains_port_credit_until_real_ack() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let mut config = cfg();
    config.capacity = 1;
    let memory = AtomicMemory::new_and_register(
        &engine,
        &clock,
        7,
        0x1000,
        256,
        config,
        AtomicPath::CacheFreeDirect,
    )
    .unwrap();
    let mut tx = OutPort::new(engine.top(), "producer");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx = InPort::<AtomicReply>::new(&engine, &clock, engine.top(), "consumer");
    memory.connect_port_tx(rx.state()).unwrap();
    engine.spawner().spawn(async move {
        tx.put(pair(1, 0x1000))?.await;
        Ok(())
    });
    let s = memory.control_handle();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        let reply = rx.start_get()?.await;
        assert_eq!(reply.completion.identity, 1);
        s.cancel(1)?;
        let before = s.snapshot();
        assert_eq!(
            (
                before.current,
                before.canceled_aftercommit_outstanding,
                before.committed_read_bytes
            ),
            (1, 1, 4)
        );
        assert_eq!(
            s.snapshot().current,
            1,
            "actual published owner retains the sole credit"
        );
        c.wait_ticks(3)?.await;
        assert_eq!(s.snapshot(), before);
        rx.finish_get();
        Ok(())
    });
    engine.run().unwrap();
    let s = memory.control_handle().snapshot();
    assert_eq!(
        (
            s.current,
            s.canceled_aftercommit,
            s.completed,
            s.committed_write_bytes
        ),
        (0, 1, 0, 4)
    );
}
#[test]
fn exact_declared_lane_and_shared_byte_constraints_use_max_not_double_billing() {
    let mut config = cfg();
    config.capacity = 4;
    config.bytes_per_tick = 8;
    config.lane_bytes_per_tick = vec![4, 4];
    let service = AtomicService::new(7, 0x1000, 256, config).unwrap();
    let a = service.try_accept(0, pair(1, 0x1000).0).unwrap();
    let b = service.try_accept(0, pair(2, 0x1004).0).unwrap();
    assert_eq!((a.deadline, b.deadline), (2, 2));
    service.commit(2, a.identity).unwrap();
    service.commit(2, b.identity).unwrap();
    let ordinary = service
        .try_accept(
            2,
            Request {
                identity: 3,
                device: 7,
                address: 0x1020,
                effect: Effect::OrdinaryRead { bytes: 8 },
            },
        )
        .unwrap();
    assert_eq!(ordinary.deadline, 3);
    service.commit(3, ordinary.identity).unwrap();
    assert_eq!(
        (
            service.snapshot().reserved_service_bytes,
            service.snapshot().committed_read_bytes,
            service.snapshot().committed_write_bytes
        ),
        (24, 16, 8)
    );
}
#[test]
fn unsupported_existing_cache_dma_or_routed_paths_reject_before_engine_registration() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    for path in [
        AtomicPath::Cache,
        AtomicPath::Dma,
        AtomicPath::RoutedOrdinary,
    ] {
        assert!(
            AtomicMemory::new_and_register(&engine, &clock, 7, 0x1000, 256, cfg(), path).is_err()
        );
    }
}
#[test]
fn actual_ordinary_atomic_and_control_ports_share_one_byte_server_and_real_poll_values() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let mut config = cfg();
    config.capacity = 4;
    config.bytes_per_tick = 8;
    config.lane_bytes_per_tick = vec![8];
    let memory = AtomicMemory::new_and_register(
        &engine,
        &clock,
        7,
        0x1000,
        256,
        config,
        AtomicPath::CacheFreeDirect,
    )
    .unwrap();
    memory
        .control_handle()
        .register_control(ControlGroup {
            invocation: 9,
            generation: 1,
            device: 7,
            signal: 0x1080,
            semaphore: 0x1084,
            participants: 2,
            publisher: 0,
            output_words: vec![0x1000],
            source_flags: SourceFlags::SplitKHgemmSc0Sc1,
        })
        .unwrap();
    let mut tx = OutPort::new(engine.top(), "source");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx = InPort::<AtomicReply>::new(&engine, &clock, engine.top(), "returns");
    memory.connect_port_tx(rx.state()).unwrap();
    let rows = Rc::new(RefCell::new(Vec::new()));
    let observed = rows.clone();
    let c = clock.clone();
    let done = Repeated::default();
    let notify = done.clone();
    engine.spawner().spawn(async move {
        for _ in 0..13 {
            let reply = rx.get()?.await;
            observed
                .borrow_mut()
                .push((c.tick_now().tick(), reply.clone()));
            notify.notify();
        }
        Ok(())
    });
    let ctrl = |identity, participant, effect| {
        AtomicPacket(Request {
            identity,
            device: 7,
            address: if matches!(
                effect,
                ControlEffect::FetchAddOne | ControlEffect::ResetSemaphore
            ) {
                0x1084
            } else {
                0x1080
            },
            effect: Effect::Control {
                invocation: 9,
                generation: 1,
                participant,
                effect,
            },
        })
    };
    let mut independent_pair = pair(2, 0x1004);
    if let Effect::Atomic { word, .. } = &mut independent_pair.0.effect {
        word.invocation = 99;
    }
    let initial = [
        AtomicPacket(Request {
            identity: 1,
            device: 7,
            address: 0x1040,
            effect: Effect::OrdinaryRead { bytes: 8 },
        }),
        independent_pair,
        ctrl(3, 1, ControlEffect::PollSignal),
    ];
    let initialize = AtomicPacket(Request {
        identity: 4,
        device: 7,
        address: 0x1000,
        effect: Effect::InitializeOutput {
            invocation: 9,
            generation: 1,
        },
    });
    let stages = vec![
        initialize,
        ctrl(5, 0, ControlEffect::PublishSignalOne),
        ctrl(6, 0, ControlEffect::PollSignal),
        ctrl(7, 0, ControlEffect::FetchAddOne),
        ctrl(8, 1, ControlEffect::PollSignal),
        ctrl(9, 1, ControlEffect::FetchAddOne),
        ctrl(10, 1, ControlEffect::ResetSemaphore),
        ctrl(11, 1, ControlEffect::ResetSignal),
    ];
    let s = memory.control_handle();
    let r = rows.clone();
    engine.spawner().spawn(async move {
        for p in initial {
            tx.put(p)?.await;
        }
        while r.borrow().len() < 3 {
            let next = done.listen();
            if r.borrow().len() < 3 {
                next.await;
            }
        }
        assert_eq!(r.borrow()[2].1.completion.control_value, Some(0));
        for p in stages {
            let next = done.listen();
            tx.put(p)?.await;
            next.await;
        }
        assert!(
            !s.control_finished(9, 1)?,
            "reset-before-output is not kernel completion"
        );
        for p in 0..2 {
            let mut packet = pair(12 + u64::from(p), 0x1000);
            if let Effect::Atomic { participant, .. } = &mut packet.0.effect {
                *participant = p;
            }
            let next = done.listen();
            tx.put(packet)?.await;
            next.await;
        }
        Ok(())
    });
    engine.run().unwrap();
    let rows = rows.borrow();
    assert_eq!(
        rows.iter().take(3).map(|r| r.0).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(rows[0].1.total_bytes(), 8);
    assert_eq!(rows[1].1.total_bytes(), 0);
    assert_eq!(rows[2].1.total_bytes(), 4);
    assert_eq!(
        (
            rows[5].1.completion.control_value,
            rows[6].1.completion.control_value,
            rows[7].1.completion.control_value,
            rows[8].1.completion.control_value
        ),
        (Some(1), Some(0), Some(1), Some(1))
    );
    let s = memory.control_handle().snapshot();
    assert_eq!((s.accepted_requests, s.completed, s.current), (13, 13, 0));
    // Independent explicit thirteen-request source ledger: ID, read, write.
    // Initialize/publish/reset are stores; only polls, fetches and pairs read.
    let expected_payloads = [
        (1, 8, 0),
        (2, 4, 4),
        (3, 4, 0),
        (4, 0, 4),
        (5, 0, 4),
        (6, 4, 0),
        (7, 4, 4),
        (8, 4, 0),
        (9, 4, 4),
        (10, 0, 4),
        (11, 0, 4),
        (12, 4, 4),
        (13, 4, 4),
    ];
    assert_eq!(
        expected_payloads
            .iter()
            .map(|(_, read, _)| read)
            .sum::<u64>(),
        40
    );
    assert_eq!(
        expected_payloads
            .iter()
            .map(|(_, _, write)| write)
            .sum::<u64>(),
        36
    );
    for ((_, reply), (id, read, write)) in rows.iter().zip(expected_payloads) {
        assert_eq!(
            (
                reply.completion.identity,
                reply.completion.read_bytes,
                reply.completion.write_bytes
            ),
            (id, read, write)
        );
    }
    assert_eq!(
        (
            s.committed_read_bytes,
            s.committed_write_bytes,
            s.reserved_service_bytes,
            s.retired_service_debt_bytes
        ),
        (40, 36, 76, 76)
    );
    assert_eq!(
        (
            s.bf16_pair_requests,
            s.bf16_element_updates,
            s.i32_fetch_add_requests,
            s.signal_poll_count
        ),
        (3, 6, 2, 3)
    );
    assert!(memory.control_handle().control_finished(9, 1).unwrap());
    println!("ATOMIC_FOUNDATION_MIXED rows={rows:?} witness={s:?}");
}
#[test]
fn actual_precommit_cancel_preserves_positive_debt_and_never_emits_a_committed_reply() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let mut config = cfg();
    config.capacity = 1;
    config.bytes_per_tick = 2;
    config.lane_bytes_per_tick = vec![2];
    let memory = AtomicMemory::new_and_register(
        &engine,
        &clock,
        7,
        0x1000,
        256,
        config,
        AtomicPath::CacheFreeDirect,
    )
    .unwrap();
    let mut tx = OutPort::new(engine.top(), "requests");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx = InPort::<AtomicReply>::new(&engine, &clock, engine.top(), "responses");
    memory.connect_port_tx(rx.state()).unwrap();
    engine.spawner().spawn(async move {
        tx.put(pair(1, 0x1000))?.await;
        tx.put(pair(2, 0x1000))?.await;
        Ok(())
    });
    let s = memory.control_handle();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        c.wait_ticks(1)?.await;
        s.cancel(1)?;
        assert_eq!(s.snapshot().canceled_precommit_outstanding, 1);
        c.wait_ticks(2)?.await;
        assert_eq!(
            (s.snapshot().current, s.snapshot().committed_read_bytes),
            (1, 0)
        );
        Ok(())
    });
    let c = clock.clone();
    engine.spawner().spawn(async move {
        let reply = rx.get()?.await;
        assert_eq!(reply.completion.identity, 2);
        assert_eq!(c.tick_now().tick(), 8);
        Ok(())
    });
    engine.run().unwrap();
    let s = memory.control_handle().snapshot();
    assert_eq!(
        (
            s.current,
            s.canceled_precommit_retired,
            s.completed,
            s.reserved_service_bytes,
            s.retired_service_debt_bytes,
            s.committed_read_bytes,
            s.committed_write_bytes
        ),
        (0, 1, 1, 16, 16, 4, 4)
    );
}

#[test]
fn actual_slow_pair_fast_ordinary_overlap_orders_without_charging_dependency_bytes_twice() {
    for cancel_second in [false, true] {
        let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
        let clock = engine.default_clock();
        let mut config = cfg();
        config.capacity = 3;
        config.bytes_per_tick = 64;
        config.lane_bytes_per_tick = vec![64];
        config.bf16_pair_per_tick = Some(1);
        let memory = AtomicMemory::new_and_register(
            &engine,
            &clock,
            7,
            0x1000,
            256,
            config,
            AtomicPath::CacheFreeDirect,
        )
        .unwrap();
        let mut tx = OutPort::new(engine.top(), "mixed");
        tx.connect(memory.port_rx()).unwrap();
        let mut rx = InPort::<AtomicReply>::new(&engine, &clock, engine.top(), "sink");
        memory.connect_port_tx(rx.state()).unwrap();
        let s = memory.control_handle();
        engine.spawner().spawn(async move {
            tx.put(pair(1, 0x1000))?.await;
            tx.put(pair(2, 0x1000))?.await;
            if cancel_second {
                s.cancel(2)?;
            }
            tx.put(AtomicPacket(Request {
                identity: 3,
                device: 7,
                address: 0x1000,
                effect: Effect::OrdinaryRead { bytes: 4 },
            }))?
            .await;
            Ok(())
        });
        let rows = Rc::new(RefCell::new(Vec::new()));
        let r = rows.clone();
        let c = clock.clone();
        engine.spawner().spawn(async move {
            for _ in 0..(if cancel_second { 2 } else { 3 }) {
                let p = rx.get()?.await;
                r.borrow_mut()
                    .push((p.completion.identity, c.tick_now().tick()));
            }
            Ok(())
        });
        engine.run().unwrap();
        assert_eq!(
            *rows.borrow(),
            if cancel_second {
                vec![(1, 1), (3, 2)]
            } else {
                vec![(1, 1), (2, 2), (3, 2)]
            }
        );
        let s = memory.control_handle().snapshot();
        assert_eq!(
            (
                s.current,
                s.reserved_service_bytes,
                s.retired_service_debt_bytes
            ),
            (0, 20, 20)
        );
        assert_eq!(
            (s.committed_read_bytes, s.committed_write_bytes),
            if cancel_second { (8, 4) } else { (12, 8) }
        );
    }
}

#[test]
fn actual_canceled_ready_reply_cannot_recycle_capacity_behind_a_stalled_publisher() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let memory = AtomicMemory::new_and_register(
        &engine,
        &clock,
        7,
        0x1000,
        256,
        cfg(),
        AtomicPath::CacheFreeDirect,
    )
    .unwrap();
    let mut tx = OutPort::new(engine.top(), "producer");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx = InPort::<AtomicReply>::new(&engine, &clock, engine.top(), "sink");
    memory.connect_port_tx(rx.state()).unwrap();
    let accepted = Rc::new(RefCell::new(Vec::new()));
    let a = accepted.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        for id in 1..=3 {
            tx.put(pair(id, 0x1000))?.await;
            a.borrow_mut().push(c.tick_now().tick());
        }
        Ok(())
    });
    let m = memory.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        c.wait_ticks(3)?.await;
        assert_eq!(m.ready_response_count(), 1);
        m.control_handle().cancel(2)?;
        for _ in 0..3 {
            assert_eq!(
                (
                    m.control_handle().snapshot().current,
                    m.control_handle()
                        .snapshot()
                        .canceled_aftercommit_outstanding,
                    m.ready_response_count()
                ),
                (2, 1, 1)
            );
            assert!(m.control_handle().cancel(2).is_err());
            c.wait_ticks(2)?.await;
        }
        Ok(())
    });
    let c = clock.clone();
    engine.spawner().spawn(async move {
        c.wait_ticks(20)?.await;
        let p = rx.get()?.await;
        assert_eq!(p.completion.identity, 1);
        let p = rx.get()?.await;
        assert_eq!(p.completion.identity, 3);
        Ok(())
    });
    engine.run().unwrap();
    assert!(accepted.borrow()[2] >= 20);
    assert_eq!(memory.ready_response_count(), 0);
    let s = memory.control_handle().snapshot();
    assert_eq!(
        (
            s.current,
            s.completed,
            s.canceled_aftercommit,
            s.committed_write_bytes
        ),
        (0, 2, 1, 12)
    );
}
