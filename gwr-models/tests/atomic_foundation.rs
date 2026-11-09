// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Caller-rate foundation controls, not an AMD operation/timing certificate.
use gwr_models::memory::atomic::*;

fn config() -> AtomicServiceConfig {
    AtomicServiceConfig {
        capacity: 4,
        bytes_per_tick: 8,
        delay_ticks: 0,
        bf16_pair_per_tick: Some(1),
        i32_fetch_add_per_tick: Some(1),
        lane_interleave_bytes: 4,
        lane_bytes_per_tick: vec![8],
    }
}
fn word(address: u64) -> AtomicWord {
    AtomicWord {
        device: 7,
        address,
        generation: 1,
        invocation: 9,
        operation: AtomicOperation::Bf16PairFadd,
        scope: AtomicScope::Agent,
        ordering: AtomicOrdering::Monotonic,
    }
}
fn request(id: u64, address: u64) -> Request {
    Request {
        identity: id,
        device: 7,
        address,
        effect: Effect::Atomic {
            word: word(address),
            participant: 0,
        },
    }
}
fn server() -> AtomicService {
    AtomicService::new(7, 0x1000, 256, config()).unwrap()
}

#[test]
fn existing_atomic_foundation_api() {
    let service = server();
    assert_eq!(service.snapshot().accepted_requests, 0);
    assert!(service.try_accept(0, request(1, 0x1000)).is_ok());
}

#[test]
fn shared_budget_same_word_commit_order_and_disjoint_words_release_before_ack() {
    let service = server();
    let a = service.try_accept(0, request(1, 0x1000)).unwrap();
    let b = service.try_accept(0, request(2, 0x1000)).unwrap();
    let c = service.try_accept(0, request(3, 0x1004)).unwrap();
    assert_eq!((a.deadline, b.deadline, c.deadline), (1, 2, 3));
    assert!(service.commit(1, b.identity).is_err());
    service.commit(1, a.identity).unwrap();
    service.commit(2, b.identity).unwrap();
    service.commit(3, c.identity).unwrap();
    // No client ACK yet: same-word successor is nonetheless permitted.
    assert_eq!(service.snapshot().committed_unacknowledged, 3);
    let d = service.try_accept(3, request(4, 0x1000)).unwrap();
    service.commit(d.deadline, d.identity).unwrap();
    assert!(
        service
            .try_accept(4, request(5, 0x1008))
            .unwrap_err()
            .to_string()
            .contains("capacity")
    );
    for ticket in [a, b, c, d] {
        service.acknowledge(ticket.identity).unwrap();
    }
    let s = service.snapshot();
    assert_eq!(
        (
            s.current,
            s.completed,
            s.committed_read_bytes,
            s.committed_write_bytes
        ),
        (0, 4, 16, 16)
    );
    assert_eq!(
        (
            s.accepted_operand_bytes,
            s.reserved_service_bytes,
            s.retired_service_debt_bytes
        ),
        (16, 32, 32)
    );
}

#[test]
fn canceled_precommit_debt_and_postcommit_effects_remain_distinct() {
    let service = server();
    let a = service.try_accept(0, request(1, 0x1000)).unwrap();
    service.cancel(a.identity).unwrap();
    assert_eq!(service.snapshot().canceled_precommit_outstanding, 1);
    assert_eq!(service.snapshot().committed_read_bytes, 0);
    service.retire_canceled(0).unwrap();
    assert_eq!(service.snapshot().current, 1);
    service.retire_canceled(1).unwrap();
    assert_eq!(
        (
            service.snapshot().current,
            service.snapshot().retired_service_debt_bytes
        ),
        (0, 8)
    );
    let b = service.try_accept(1, request(2, 0x1000)).unwrap();
    service.commit(b.deadline, b.identity).unwrap();
    service.cancel(b.identity).unwrap();
    let s = service.snapshot();
    assert_eq!(
        (
            s.canceled_precommit_retired,
            s.canceled_aftercommit_outstanding,
            s.current,
            s.committed_read_bytes,
            s.committed_write_bytes
        ),
        (1, 1, 1, 4, 4)
    );
    assert_eq!(s.reserved_service_bytes, s.retired_service_debt_bytes);
    assert_eq!(
        s.accepted_requests,
        s.current + s.completed + s.canceled_precommit_retired + s.canceled_aftercommit
    );
}

#[test]
fn same_tick_cancel_before_and_after_commit_have_different_actual_effects() {
    let first = server();
    let a = first.try_accept(0, request(1, 0x1000)).unwrap();
    first.cancel(a.identity).unwrap();
    first.retire_canceled(a.deadline).unwrap();
    assert!(first.commit(a.deadline, a.identity).is_err());
    assert_eq!(first.snapshot().committed_read_bytes, 0);
    let second = server();
    let b = second.try_accept(0, request(1, 0x1000)).unwrap();
    second.commit(b.deadline, b.identity).unwrap();
    second.cancel(b.identity).unwrap();
    assert_eq!(
        (
            second.snapshot().committed_read_bytes,
            second.snapshot().committed_write_bytes
        ),
        (4, 4)
    );
}

#[test]
fn invalid_requests_reject_after_happy_baseline_without_any_reservation() {
    let service = server();
    let a = service.try_accept(0, request(1, 0x1000)).unwrap();
    service.commit(a.deadline, a.identity).unwrap();
    service.acknowledge(a.identity).unwrap();
    let before = service.debug_state();
    for mut bad in [request(2, 0x1002), request(2, 0x1100), request(2, 0x1000)] {
        if bad.address == 0x1000 {
            bad.device = 8;
        }
        assert!(service.try_accept(1, bad).is_err());
        assert_eq!(service.debug_state(), before);
    }
    let mut missing = config();
    missing.bf16_pair_per_tick = None;
    let unavailable = AtomicService::new(7, 0x1000, 256, missing).unwrap();
    let before = unavailable.debug_state();
    assert!(
        unavailable
            .try_accept(0, request(1, 0x1000))
            .unwrap_err()
            .to_string()
            .contains("capability")
    );
    assert_eq!(unavailable.debug_state(), before);
    let before = service.debug_state();
    assert!(service.try_accept(u64::MAX, request(2, 0x1000)).is_err());
    assert_eq!(service.debug_state(), before);
    let mut invalid = config();
    invalid.bytes_per_tick = 0;
    assert!(AtomicService::new(7, 0x1000, 256, invalid).is_err());
}

#[test]
fn live_word_generation_is_owned_until_commit_or_canceled_debt_retirement() {
    let service = server();
    let first = service.try_accept(0, request(1, 0x1000)).unwrap();
    let mut foreign = request(2, 0x1000);
    if let Effect::Atomic { ref mut word, .. } = foreign.effect {
        word.generation = 2;
    }
    let before = service.debug_state();
    assert!(service.try_accept(0, foreign.clone()).is_err());
    assert_eq!(service.debug_state(), before);
    service.cancel(1).unwrap();
    let before = service.debug_state();
    assert!(service.try_accept(0, foreign.clone()).is_err());
    assert_eq!(service.debug_state(), before);
    service.retire_canceled(first.deadline).unwrap();
    let second = service.try_accept(first.deadline, foreign).unwrap();
    service.commit(second.deadline, second.identity).unwrap();
    service.acknowledge(2).unwrap();
    assert_eq!(service.snapshot().committed_write_bytes, 4);
}

fn group() -> ControlGroup {
    ControlGroup {
        invocation: 9,
        generation: 1,
        device: 7,
        signal: 0x1080,
        semaphore: 0x1084,
        participants: 2,
        publisher: 0,
        output_words: vec![0x1000],
        source_flags: SourceFlags::SplitKHgemmSc0Sc1,
    }
}
fn control(id: u64, effect: ControlEffect, participant: u32) -> Request {
    let g = group();
    let address = match effect {
        ControlEffect::FetchAddOne | ControlEffect::ResetSemaphore => g.semaphore,
        _ => g.signal,
    };
    Request {
        identity: id,
        device: 7,
        address,
        effect: Effect::Control {
            invocation: g.invocation,
            generation: g.generation,
            participant,
            effect,
        },
    }
}
fn complete(service: &AtomicService, at: &mut u64, request: Request) -> Completion {
    let ticket = service.try_accept(*at, request).unwrap();
    *at = ticket.deadline;
    let result = service.commit(*at, ticket.identity).unwrap();
    service.acknowledge(ticket.identity).unwrap();
    result
}

#[test]
fn real_control_zero_poll_publication_last_arrival_reset_and_final_join_are_distinct() {
    let service = server();
    service.register_control(group()).unwrap();
    let mut t = 0;
    assert_eq!(
        complete(&service, &mut t, control(1, ControlEffect::PollSignal, 1)).control_value,
        Some(0)
    );
    assert!(
        service
            .try_accept(t, control(2, ControlEffect::PublishSignalOne, 0))
            .is_err()
    );
    // This API records the actual completed C-zero milestone, not a guessed
    // reset.
    complete(
        &service,
        &mut t,
        Request {
            identity: 2,
            device: 7,
            address: 0x1000,
            effect: Effect::InitializeOutput {
                invocation: 9,
                generation: 1,
            },
        },
    );
    complete(
        &service,
        &mut t,
        control(3, ControlEffect::PublishSignalOne, 0),
    );
    for (id, p) in [(4, 0), (6, 1)] {
        assert_eq!(
            complete(&service, &mut t, control(id, ControlEffect::PollSignal, p)).control_value,
            Some(1)
        );
        assert_eq!(
            complete(
                &service,
                &mut t,
                control(id + 1, ControlEffect::FetchAddOne, p)
            )
            .control_value,
            Some(p)
        );
    }
    let mut early_output = request(8, 0x1000);
    early_output.effect = Effect::Atomic {
        word: word(0x1000),
        participant: 1,
    };
    let before = service.debug_state();
    assert!(service.try_accept(t, early_output.clone()).is_err());
    assert_eq!(service.debug_state(), before);
    complete(
        &service,
        &mut t,
        control(8, ControlEffect::ResetSemaphore, 1),
    );
    early_output.identity = 9;
    let before = service.debug_state();
    assert!(service.try_accept(t, early_output).is_err());
    assert_eq!(service.debug_state(), before);
    complete(&service, &mut t, control(9, ControlEffect::ResetSignal, 1));
    assert!(
        !service.control_finished(9, 1).unwrap(),
        "reset must not certify output commits"
    );
    for p in 0..2 {
        let mut r = request(10 + u64::from(p), 0x1000);
        r.effect = Effect::Atomic {
            word: word(0x1000),
            participant: p,
        };
        complete(&service, &mut t, r);
    }
    assert!(service.control_finished(9, 1).unwrap());
    let before = service.debug_state();
    assert!(service.register_control(group()).is_err());
    assert_eq!(service.debug_state(), before);
    let mut next = group();
    next.generation = 2;
    service.register_control(next).unwrap();
}

#[test]
fn positive_retained_poll_debt_allows_same_tick_work_but_cannot_zeno() {
    let mut cfg = config();
    cfg.capacity = 128;
    cfg.bytes_per_tick = 400;
    cfg.lane_bytes_per_tick = vec![400];
    let service = AtomicService::new(7, 0x1000, 256, cfg).unwrap();
    service.register_control(group()).unwrap();
    let mut same_tick = 0;
    let mut last = 0;
    let tickets: Vec<_> = (1..=101)
        .map(|id| {
            service
                .try_accept(0, control(id, ControlEffect::PollSignal, 1))
                .unwrap()
        })
        .collect();
    for ticket in tickets {
        if ticket.deadline == 1 {
            same_tick += 1;
        }
        last = ticket.deadline;
        assert_eq!(
            service.commit(last, ticket.identity).unwrap().control_value,
            Some(0)
        );
        service.acknowledge(ticket.identity).unwrap();
    }
    assert_eq!((same_tick, last), (100, 2));
    assert_eq!(service.snapshot().committed_read_bytes, 404);
    assert_eq!(service.snapshot().signal_poll_count, 101);
}

#[test]
fn every_fetch_reply_gates_output_and_old_reply_blocks_disjoint_generation_replacement() {
    let service = server();
    service.register_control(group()).unwrap();
    let mut t = 0;
    complete(
        &service,
        &mut t,
        Request {
            identity: 1,
            device: 7,
            address: 0x1000,
            effect: Effect::InitializeOutput {
                invocation: 9,
                generation: 1,
            },
        },
    );
    complete(
        &service,
        &mut t,
        control(2, ControlEffect::PublishSignalOne, 0),
    );
    let old = service
        .try_accept(t, control(3, ControlEffect::PollSignal, 0))
        .unwrap();
    t = old.deadline;
    assert_eq!(service.commit(t, 3).unwrap().control_value, Some(1)); // retained unacknowledged duplicate poll
    complete(&service, &mut t, control(4, ControlEffect::PollSignal, 0));
    let fetch = service
        .try_accept(t, control(5, ControlEffect::FetchAddOne, 0))
        .unwrap();
    t = fetch.deadline;
    assert_eq!(service.commit(t, 5).unwrap().control_value, Some(0));
    let mut output = request(6, 0x1000);
    let before = service.debug_state();
    assert!(service.try_accept(t, output.clone()).is_err());
    assert_eq!(service.debug_state(), before);
    service.acknowledge(5).unwrap();
    complete(&service, &mut t, output.clone());
    complete(&service, &mut t, control(7, ControlEffect::PollSignal, 1));
    complete(&service, &mut t, control(8, ControlEffect::FetchAddOne, 1));
    complete(
        &service,
        &mut t,
        control(9, ControlEffect::ResetSemaphore, 1),
    );
    complete(&service, &mut t, control(10, ControlEffect::ResetSignal, 1));
    output.identity = 11;
    if let Effect::Atomic { participant, .. } = &mut output.effect {
        *participant = 1;
    }
    complete(&service, &mut t, output);
    assert!(service.control_finished(9, 1).unwrap());
    let mut next = group();
    next.generation = 2;
    next.signal = 0x10a0;
    next.semaphore = 0x10a4;
    next.output_words = vec![0x1040];
    let before = service.debug_state();
    assert!(
        service
            .register_control(next.clone())
            .unwrap_err()
            .to_string()
            .contains("outstanding")
    );
    assert_eq!(service.debug_state(), before);
    service.acknowledge(3).unwrap();
    service.register_control(next).unwrap();
    let before = service.debug_state();
    assert!(service.acknowledge(3).is_err());
    assert_eq!(service.debug_state(), before);
    let mut fresh_fetch = control(12, ControlEffect::FetchAddOne, 0);
    fresh_fetch.address = 0x10a4;
    if let Effect::Control { generation, .. } = &mut fresh_fetch.effect {
        *generation = 2;
    }
    let before = service.debug_state();
    assert!(service.try_accept(t, fresh_fetch).is_err());
    assert_eq!(service.debug_state(), before); // old successful poll cannot authorize next-generation increment
}

#[test]
fn control_foreign_generation_premature_reset_and_ordinary_word_alias_are_zero_effects() {
    let service = server();
    service.register_control(group()).unwrap();
    let mut t = 0;
    complete(&service, &mut t, control(1, ControlEffect::PollSignal, 0));
    let before = service.debug_state();
    for bad in [
        control(2, ControlEffect::FetchAddOne, 0),
        control(2, ControlEffect::ResetSignal, 0),
        Request {
            identity: 2,
            device: 7,
            address: 0x1080,
            effect: Effect::OrdinaryWrite { bytes: 4 },
        },
        Request {
            identity: 2,
            device: 7,
            address: 0x1080,
            effect: Effect::Control {
                invocation: 9,
                generation: 2,
                participant: 0,
                effect: ControlEffect::PollSignal,
            },
        },
    ] {
        assert!(service.try_accept(t, bad).is_err());
        assert_eq!(service.debug_state(), before);
    }
}
