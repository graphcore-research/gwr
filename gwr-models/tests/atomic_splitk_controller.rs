// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Unexecuted protocol controls, not a source-streaming or numerical witness.
use futures::channel::oneshot;
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::types::SimError;
use gwr_models::memory::atomic::*;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::{Memory, MemoryConfig};
fn service() -> AtomicServiceConfig {
    AtomicServiceConfig {
        capacity: 4,
        bytes_per_tick: 16,
        delay_ticks: 0,
        bf16_pair_per_tick: Some(1),
        i32_fetch_add_per_tick: Some(1),
        lane_interleave_bytes: 4,
        lane_bytes_per_tick: vec![16],
    }
}
fn group() -> ControlGroup {
    ControlGroup {
        invocation: 9,
        generation: 1,
        device: 7,
        signal: 4224,
        semaphore: 4228,
        participants: 2,
        publisher: 0,
        output_words: vec![4096],
        source_flags: SourceFlags::SplitKHgemmSc0Sc1,
    }
}
fn control(p: u32, effect: ControlEffect) -> Effect {
    Effect::Control {
        invocation: 9,
        generation: 1,
        participant: p,
        effect,
    }
}
fn pair(p: u32) -> Effect {
    Effect::Atomic {
        participant: p,
        word: AtomicWord {
            device: 7,
            address: 4096,
            invocation: 9,
            generation: 1,
            operation: AtomicOperation::Bf16PairFadd,
            scope: AtomicScope::Agent,
            ordering: AtomicOrdering::Monotonic,
        },
    }
}
#[test]
fn actual_zero_polls_arrival_permutations_dynamic_last_resets_and_all_output_ack_join() {
    for slow in [0u32, 1] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let memory = Memory::<MemoryAccess>::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "m",
            MemoryConfig::new(4096, 256, 16, 0)
                .with_arrival_aware_service(4)
                .with_service_lanes(4, vec![16])
                .with_atomic_service(7, service()),
        )
        .unwrap();
        let mut idle = OutPort::new(engine.top(), "ordinary_idle");
        idle.connect(memory.port_rx()).unwrap();
        let sink = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "ordinary_sink");
        memory.connect_port_tx(sink.state()).unwrap();
        let authority = memory.atomic_control().unwrap();
        let controller =
            SplitKController::new(group(), memory.atomic_target().unwrap(), authority.clone())
                .unwrap();
        let c = clock.clone();
        let task = controller.clone();
        engine.spawner().spawn(async move {
            c.wait_ticks(5)?.await;
            task.execute(
                4096,
                Effect::InitializeOutput {
                    invocation: 9,
                    generation: 1,
                },
            )
            .await?;
            task.execute(4224, control(0, ControlEffect::PublishSignalOne))
                .await?;
            Ok(())
        });
        let (fetch0, received0) = oneshot::channel();
        let (fetch1, received1) = oneshot::channel();
        let (output0, done0) = oneshot::channel();
        let (output1, done1) = oneshot::channel();
        for (p, fetched, output) in [(0, fetch0, output0), (1, fetch1, output1)] {
            let task = controller.clone();
            let c = clock.clone();
            engine.spawner().spawn(async move {
                let signal = task
                    .execute(4224, control(p, ControlEffect::PollSignal))
                    .await?;
                assert_eq!(signal.control_value, Some(1));
                if p == slow {
                    c.wait_ticks(5)?.await;
                }
                let old = task
                    .execute(4228, control(p, ControlEffect::FetchAddOne))
                    .await?
                    .control_value
                    .unwrap();
                fetched
                    .send(old)
                    .map_err(|_| SimError("actual fetch observer dropped".into()))?;
                task.execute(4096, pair(p)).await?;
                output
                    .send(())
                    .map_err(|_| SimError("actual output join dropped".into()))?;
                Ok(())
            });
        }
        let task = controller.clone();
        engine.spawner().spawn(async move {
            let a = received0
                .await
                .map_err(|_| SimError("fetch0 failed".into()))?;
            let b = received1
                .await
                .map_err(|_| SimError("fetch1 failed".into()))?;
            assert_eq!((a, b), if slow == 0 { (1, 0) } else { (0, 1) });
            assert!(task.verify_final_join().is_err());
            task.execute(4228, control(u32::MAX, ControlEffect::ResetSemaphore))
                .await?;
            task.execute(4224, control(u32::MAX, ControlEffect::ResetSignal))
                .await?;
            done0.await.map_err(|_| SimError("output0 failed".into()))?;
            done1.await.map_err(|_| SimError("output1 failed".into()))?;
            task.verify_final_join()?;
            Ok(())
        });
        engine.run().unwrap();
        let observed = controller.observation();
        assert_eq!(observed.last, Some(slow));
        assert!(
            observed
                .participants
                .values()
                .all(|p| p.zero_polls > 0 && p.output_acks.contains(&4096))
        );
        let polls = observed.participants.values().map(|p| p.polls).sum::<u64>();
        let s = authority.snapshot();
        assert_eq!(
            (
                s.current,
                s.completed,
                s.bf16_pair_requests,
                s.i32_fetch_add_requests
            ),
            (0, polls + 8, 2, 2)
        );
        assert_eq!(
            (s.committed_read_bytes, s.committed_write_bytes),
            (16 + 4 * polls, 32)
        );
        println!("ACTUAL_SPLITK_CONTROL {observed:?} physical={s:?}");
    }
}
#[test]
fn actual_generation_role_preflight_is_stable_and_cannot_guess_last_reset() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let memory = Memory::<MemoryAccess>::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "m",
        MemoryConfig::new(4096, 256, 16, 0)
            .with_arrival_aware_service(4)
            .with_service_lanes(4, vec![16])
            .with_atomic_service(7, service()),
    )
    .unwrap();
    let authority = memory.atomic_control().unwrap();
    let c =
        SplitKController::new(group(), memory.atomic_target().unwrap(), authority.clone()).unwrap();
    c.validate_role(4224, control(0, ControlEffect::PollSignal))
        .unwrap();
    c.validate_role(4228, control(u32::MAX, ControlEffect::ResetSemaphore))
        .unwrap();
    let before = authority.snapshot();
    for _ in 0..3 {
        c.validate_role(4224, control(0, ControlEffect::PollSignal))
            .unwrap();
    }
    assert!(
        c.validate_role(
            4224,
            Effect::Control {
                invocation: 9,
                generation: 2,
                participant: 0,
                effect: ControlEffect::PollSignal
            }
        )
        .is_err()
    );
    assert!(
        c.validate_role(4224, control(u32::MAX, ControlEffect::ResetSemaphore))
            .is_err()
    );
    assert_eq!(authority.snapshot(), before);
    let error = futures::executor::block_on(
        c.execute(4228, control(u32::MAX, ControlEffect::ResetSemaphore)),
    )
    .unwrap_err();
    assert!(error.to_string().contains("actual last fetch receipt"));
    assert_eq!(authority.snapshot(), before);
    assert!(c.observation().failed);
    assert!(c.verify_final_join().is_err());
}
