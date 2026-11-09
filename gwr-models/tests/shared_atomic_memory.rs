// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Unexecuted real-port controls; rates are caller assumptions, not hardware.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::future::{Either, select};
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::{Routable, TotalBytes};
use gwr_engine::types::{AccessType, DeviceId, SimError};
use gwr_models::memory::atomic::*;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_track::id::Unique;
fn config(capacity: usize) -> AtomicServiceConfig {
    AtomicServiceConfig {
        capacity,
        bytes_per_tick: 64,
        delay_ticks: 0,
        bf16_pair_per_tick: Some(1),
        i32_fetch_add_per_tick: Some(1),
        lane_interleave_bytes: 4,
        lane_bytes_per_tick: vec![64],
    }
}
fn model(capacity: usize) -> MemoryConfig {
    MemoryConfig::new(4096, 256, 64, 0)
        .with_arrival_aware_service(capacity)
        .with_service_lanes(4, vec![64])
        .with_atomic_service(7, config(capacity))
}
fn pair(address: u64) -> Effect {
    Effect::Atomic {
        participant: 0,
        word: AtomicWord {
            device: 7,
            address,
            invocation: 99,
            generation: 1,
            operation: AtomicOperation::Bf16PairFadd,
            scope: AtomicScope::Agent,
            ordering: AtomicOrdering::Monotonic,
        },
    }
}
fn access(engine: &Engine, kind: AccessType, address: u64) -> MemoryAccess {
    MemoryAccess::new(
        engine.top(),
        kind,
        4,
        address,
        8192,
        DeviceId(7),
        DeviceId(1),
        16,
    )
}
#[test]
fn happy_first_declaration_rejects_conflicting_duplicate_budget_and_capability_before_activity() {
    model(3).validate().unwrap();
    let mut bad = config(3);
    bad.bytes_per_tick = 32;
    bad.lane_bytes_per_tick = vec![32];
    assert!(
        MemoryConfig::new(4096, 256, 64, 0)
            .with_arrival_aware_service(3)
            .with_service_lanes(4, vec![64])
            .with_atomic_service(7, bad)
            .validate()
            .unwrap_err()
            .to_string()
            .contains("conflicts")
    );
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let memory =
        Memory::<MemoryAccess>::new_and_register(&engine, &clock, engine.top(), "m", model(3))
            .unwrap();
    let target = memory.atomic_target().unwrap();
    let before = memory.atomic_control().unwrap().snapshot();
    target.validate_effect(7, 4096, pair(4096)).unwrap();
    for _ in 0..3 {
        target.validate_effect(7, 4096, pair(4096)).unwrap();
    }
    assert_eq!(memory.atomic_control().unwrap().snapshot(), before);
    assert!(target.validate_effect(8, 4096, pair(4096)).is_err());
    assert!(target.validate_effect(7, 4098, pair(4098)).is_err());
    assert!(
        target
            .validate_effect(7, 4096, Effect::OrdinaryRead { bytes: 4 })
            .is_err()
    );
    assert!(
        target
            .validate_effect(
                7,
                4224,
                Effect::Control {
                    invocation: 9,
                    generation: 1,
                    participant: 0,
                    effect: ControlEffect::PollSignal
                }
            )
            .is_err()
    );
    assert_eq!(memory.atomic_control().unwrap().snapshot(), before);
}
#[test]
fn actual_two_pairs_then_ordinary_read_keep_shared_debt_and_original_response_identity() {
    for cancel_second in [false, true] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let memory =
            Memory::<MemoryAccess>::new_and_register(&engine, &clock, engine.top(), "m", model(3))
                .unwrap();
        let mut tx = OutPort::new(engine.top(), "ordinary");
        tx.connect(memory.port_rx()).unwrap();
        let mut rx = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "ordinary_reply");
        memory.connect_port_tx(rx.state()).unwrap();
        let p = access(&engine, AccessType::ReadRequest, 4096);
        let original = p.id();
        let header = p.total_bytes();
        assert_eq!(header, 16);
        let target = memory.atomic_target().unwrap();
        let rows = Rc::new(RefCell::new(Vec::new()));
        for ordinal in [1, 2] {
            let target = target.clone();
            let c = clock.clone();
            let rows = rows.clone();
            engine.spawner().spawn(async move {
                let result = target.execute(7, 4096, pair(4096)).await;
                if cancel_second && ordinal == 2 {
                    assert!(result.unwrap_err().to_string().contains("canceled"));
                } else {
                    let result = result?;
                    assert_eq!(result.identity, ordinal);
                    rows.borrow_mut().push((ordinal, c.tick_now().tick()));
                }
                Ok(())
            });
        }
        let control = memory.atomic_control().unwrap();
        let c = clock.clone();
        engine.spawner().spawn(async move {
            c.wait_ticks(1)?.await;
            assert_eq!(
                control.snapshot().accepted_requests,
                2,
                "two real typed inputs already admitted"
            );
            if cancel_second {
                control.cancel_owner(2)?;
                assert_eq!(control.snapshot().canceled_precommit_outstanding, 1);
            }
            tx.put(p)?.await;
            Ok(())
        });
        let c = clock.clone();
        engine.spawner().spawn(async move {
            let p = rx.get()?.await;
            assert_eq!(p.id(), original);
            assert_eq!(p.dst_addr(), 4096);
            assert_eq!(p.src_addr(), 8192);
            assert_eq!(p.access_type(), AccessType::ReadResponse);
            assert_eq!(p.total_bytes(), 20);
            assert_eq!(c.tick_now().tick(), 2);
            Ok(())
        });
        engine.run().unwrap();
        let s = memory.atomic_control().unwrap().snapshot();
        assert_eq!(s.current, 0);
        assert_eq!(
            (s.reserved_service_bytes, s.retired_service_debt_bytes),
            (20, 20)
        );
        assert_eq!(
            (memory.bytes_read(), memory.bytes_written()),
            if cancel_second { (8, 4) } else { (12, 8) }
        );
        assert_eq!(
            *rows.borrow(),
            if cancel_second {
                vec![(1, 1)]
            } else {
                vec![(1, 1), (2, 2)]
            }
        );
    }
}
#[test]
fn real_received_reply_cancellation_drains_ack_without_successful_continuation() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let memory =
        Memory::<MemoryAccess>::new_and_register(&engine, &clock, engine.top(), "m", model(1))
            .unwrap();
    // Even an idle ordinary input remains a real connected port. It does not
    // reserve the only capacity credit and block typed intake.
    let mut idle = OutPort::new(engine.top(), "idle");
    idle.connect(memory.port_rx()).unwrap();
    let rx = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "unused_reply");
    memory.connect_port_tx(rx.state()).unwrap();
    let target = memory.atomic_target().unwrap();
    let control = memory.atomic_control().unwrap();
    let continued = Rc::new(Cell::new(false));
    let done = continued.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        let receipt = target.receive(7, 4096, pair(4096)).await?;
        assert_eq!(receipt.completion().identity, 1);
        assert_eq!(receipt.completion().control_value, None);
        assert_eq!(control.snapshot().current, 1);
        control.cancel_owner(1)?;
        let before = control.snapshot();
        assert_eq!(
            (
                before.canceled_aftercommit_outstanding,
                before.committed_write_bytes
            ),
            (1, 4)
        );
        c.wait_ticks(3)?.await;
        assert_eq!(control.snapshot(), before);
        match receipt.acknowledge().await {
            Err(error) => assert!(error.to_string().contains("no continuation authority")),
            Ok(_) => done.set(true),
        }
        Ok(())
    });
    engine.run().unwrap();
    assert!(!continued.get());
    let s = memory.atomic_control().unwrap().snapshot();
    assert_eq!(
        (
            s.current,
            s.completed,
            s.canceled_aftercommit,
            s.committed_write_bytes
        ),
        (0, 0, 1, 4)
    );
}
#[test]
fn posted_commit_retires_but_real_read_and_write_np_ack_retain_credit() {
    for kind in [
        AccessType::WriteRequest,
        AccessType::ReadRequest,
        AccessType::WriteNonPostedRequest,
    ] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let memory =
            Memory::<MemoryAccess>::new_and_register(&engine, &clock, engine.top(), "m", model(1))
                .unwrap();
        let mut tx = OutPort::new(engine.top(), "source");
        tx.connect(memory.port_rx()).unwrap();
        let mut rx = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "sink");
        memory.connect_port_tx(rx.state()).unwrap();
        let p = access(&engine, kind, 4096);
        engine.spawner().spawn(async move {
            tx.put(p)?.await;
            Ok(())
        });
        let control = memory.atomic_control().unwrap();
        let c = clock.clone();
        engine.spawner().spawn(async move {
            c.wait_ticks(3)?.await;
            assert_eq!(control.snapshot().committed_requests, 1);
            if kind == AccessType::WriteRequest {
                assert_eq!(
                    (control.snapshot().current, control.snapshot().completed),
                    (0, 1)
                );
            } else {
                assert_eq!(control.snapshot().current, 1);
                let p = rx.start_get()?.await;
                assert!(matches!(
                    p.access_type(),
                    AccessType::ReadResponse | AccessType::WriteNonPostedResponse
                ));
                c.wait_ticks(2)?.await;
                assert_eq!(control.snapshot().current, 1);
                rx.finish_get();
            }
            Ok(())
        });
        engine.run().unwrap();
        assert_eq!(memory.atomic_control().unwrap().snapshot().current, 0);
    }
}
#[test]
fn canceled_ready_response_does_not_recycle_memory_credit_behind_real_stalled_reply() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let memory =
        Memory::<MemoryAccess>::new_and_register(&engine, &clock, engine.top(), "m", model(2))
            .unwrap();
    let mut idle = OutPort::new(engine.top(), "idle");
    idle.connect(memory.port_rx()).unwrap();
    let rx = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "unused");
    memory.connect_port_tx(rx.state()).unwrap();
    let target = memory.atomic_target().unwrap();
    let first = Rc::new(RefCell::new(None));
    let slot = first.clone();
    let t = target.clone();
    engine.spawner().spawn(async move {
        *slot.borrow_mut() = Some(t.receive(7, 4096, pair(4096)).await?);
        Ok(())
    });
    let t = target.clone();
    engine.spawner().spawn(async move {
        assert!(
            t.execute(7, 4100, pair(4100))
                .await
                .unwrap_err()
                .to_string()
                .contains("canceled")
        );
        Ok(())
    });
    let third = Rc::new(Cell::new(false));
    let done = third.clone();
    let t = target.clone();
    engine.spawner().spawn(async move {
        t.execute(7, 4104, pair(4104)).await?;
        done.set(true);
        Ok(())
    });
    let control = memory.atomic_control().unwrap();
    let c = clock.clone();
    let done = third.clone();
    engine.spawner().spawn(async move {
        c.wait_ticks(3)?.await;
        assert_eq!(
            (
                control.snapshot().current,
                control.snapshot().committed_requests
            ),
            (2, 2)
        );
        control.cancel_owner(2)?;
        c.wait_ticks(5)?.await;
        assert_eq!(control.snapshot().current, 2);
        assert!(!done.get());
        let receipt = first
            .borrow_mut()
            .take()
            .ok_or_else(|| SimError("first actual receipt missing".into()))?;
        receipt.acknowledge().await?;
        Ok(())
    });
    engine.run().unwrap();
    assert!(third.get());
    let s = memory.atomic_control().unwrap().snapshot();
    assert_eq!(
        (s.current, s.completed, s.canceled_aftercommit, s.peak),
        (0, 2, 1, 2)
    );
}
#[test]
fn abandoned_receive_cannot_reuse_the_internal_pending_put_or_panic_port_state() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let memory =
        Memory::<MemoryAccess>::new_and_register(&engine, &clock, engine.top(), "m", model(2))
            .unwrap();
    let mut idle = OutPort::new(engine.top(), "idle");
    idle.connect(memory.port_rx()).unwrap();
    let rx = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "unused");
    memory.connect_port_tx(rx.state()).unwrap();
    let target = memory.atomic_target().unwrap();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        // First future reaches enqueue/Pending then is dropped. The issuer is
        // an independent actor: neither this Drop nor the next call owns tx.
        let future = Box::pin(target.receive(7, 4096, pair(4096)));
        let one_poll = Box::pin(async {});
        match select(future, one_poll).await {
            Either::Right(((), pending)) => drop(pending),
            _ => panic!("reply cannot exist before real service"),
        }
        let result = target.execute(7, 4100, pair(4100)).await?;
        assert_eq!(result.control_value, None);
        assert!(c.tick_now().tick() > 0);
        Ok(())
    });
    engine.run().unwrap();
    let s = memory.atomic_control().unwrap().snapshot();
    assert_eq!(s.current, 0);
    assert_eq!(s.completed, 1);
}

#[test]
fn actual_successful_direct_request_and_reply_identity_cannot_be_forged_by_public_task_config() {
    use gwr_models::processing_element::task::AtomicTaskConfig;
    use gwr_models::processing_element::{ProcessingElement, ProcessingElementConfig};
    let cfg = ProcessingElementConfig {
        block_execution: None,
        num_active_requests: 2,
        lsu_access_bytes: 4,
        overhead_size_bytes: 0,
        sram_bytes: 64,
        boundary_payload_service: None,
        local_payload_service: None,
        compute: yaml_serde::from_str(
            "engines: {alu: {capabilities: [{class: add, ops_per_tick: 64}]}}",
        )
        .unwrap(),
        operation_classes: yaml_serde::from_str("add: {operations: [add], arity: 2}").unwrap(),
    };
    for (wrong_request, wrong_reply) in [(false, false), (true, false), (false, true)] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let memory =
            Memory::<MemoryAccess>::new_and_register(&engine, &clock, engine.top(), "m", model(2))
                .unwrap();
        let mut map = gwr_models::memory::memory_map::MemoryMap::new();
        map.insert(4096, 256, DeviceId(7)).unwrap();
        let map = Rc::new(map);
        let pe = ProcessingElement::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "pe",
            &map,
            &cfg,
            DeviceId(1),
        )
        .unwrap();
        let other = ProcessingElement::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "other",
            &map,
            &cfg,
            DeviceId(2),
        )
        .unwrap();
        let dummy_in = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "wrong_input");
        let dummy_reply = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "wrong_reply");
        pe.connect_port_tx(if wrong_request {
            dummy_in.state()
        } else {
            memory.port_rx()
        })
        .unwrap();
        memory
            .connect_port_tx(if wrong_reply {
                dummy_reply.state()
            } else {
                pe.port_rx()
            })
            .unwrap();
        let config = AtomicTaskConfig::new(
            "a".into(),
            memory.atomic_target().unwrap(),
            7,
            4096,
            pair(4096),
        )
        .unwrap()
        .with_physical_issuer(&pe);
        if wrong_request || wrong_reply {
            assert!(
                pe.preflight_atomic_task(&config)
                    .unwrap_err()
                    .to_string()
                    .contains("actual direct ordinary request AND reply")
            );
        } else {
            pe.preflight_atomic_task(&config).unwrap();
            assert!(
                other
                    .preflight_atomic_task(&config)
                    .unwrap_err()
                    .to_string()
                    .contains("foreign PE")
            );
        }
        assert_eq!(
            memory
                .atomic_control()
                .unwrap()
                .snapshot()
                .accepted_requests,
            0
        );
        assert_eq!(pe.memory_traffic_bytes(), (0, 0));
        assert_eq!(pe.peak_sram_bytes(), 0);
    }
}
