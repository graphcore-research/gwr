// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
// Target-only uncompiled tests. Logical local debt is not byte-owning SRAM use.
use gwr_engine::engine::Engine;

use super::{LocalPayloadDirection as Direction, LocalPayloadService as Service};

#[test]
fn valid_rate_zero_payload_and_actual_tick_contract() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let service = Service::new(4, &clock).unwrap();
    let before = service.snapshot();
    let zero = service.reserve(Direction::Read, 0).unwrap();
    assert_eq!(zero.deadline_tick(), 0);
    drop(zero);
    assert_eq!(service.snapshot(), before);
    assert!(
        Service::new(0, &clock)
            .err()
            .unwrap()
            .to_string()
            .contains("positive")
    );
    assert!(
        Service::new(4, &engine.clock_ghz(2.0))
            .err()
            .unwrap()
            .to_string()
            .contains("ticks")
    );
}

#[test]
fn read_and_write_share_exact_fractional_debt_without_per_ticket_rounding() {
    let mut engine = Engine::default();
    let service = Service::new(4, &engine.default_clock()).unwrap();
    let first = service.reserve(Direction::Read, 1).unwrap();
    let second = service.reserve(Direction::Write, 3).unwrap();
    let third = service.reserve(Direction::Read, 1).unwrap();
    assert_eq!(
        [
            first.deadline_tick(),
            second.deadline_tick(),
            third.deadline_tick()
        ],
        [1, 1, 2]
    );
    assert_eq!(service.snapshot().admitted_read_bytes, 2);
    assert_eq!(service.snapshot().admitted_write_bytes, 3);
    drop((first, second, third));
    let snapshot = service.snapshot();
    assert_eq!(
        (snapshot.canceled_read_bytes, snapshot.canceled_write_bytes),
        (2, 3)
    );
    assert_eq!(
        (
            snapshot.completed_read_bytes,
            snapshot.completed_write_bytes
        ),
        (0, 0)
    );
}

#[test]
fn dropping_accepted_ticket_retains_debt_and_never_completes_local_use() {
    let mut engine = Engine::default();
    let service = Service::new(2, &engine.default_clock()).unwrap();
    let abandoned = service.reserve(Direction::Write, 5).unwrap();
    assert_eq!(abandoned.deadline_tick(), 3);
    drop(abandoned);
    assert_eq!(service.snapshot().completed_write_bytes, 0);
    assert_eq!(service.snapshot().canceled_write_bytes, 5);
    let retry = service.reserve(Direction::Read, 1).unwrap();
    assert_eq!(retry.deadline_tick(), 3);
    drop(retry);
    assert_eq!(service.snapshot().canceled_read_bytes, 1);
}

#[test]
fn debt_deadline_and_counter_overflow_reject_before_state_commit() {
    let mut engine = Engine::default();
    let service = Service::new(4, &engine.default_clock()).unwrap();
    let held = service.reserve(Direction::Read, usize::MAX).unwrap();
    let before = service.snapshot();
    assert!(service.reserve(Direction::Read, 1).is_err());
    assert_eq!(service.snapshot(), before);
    drop(held);
    // A second rate-one resource reaches u64's deadline without poisoning
    // state.
    let deadline = Service::new(1, &engine.default_clock()).unwrap();
    let held = deadline.reserve(Direction::Write, usize::MAX).unwrap();
    let before = deadline.snapshot();
    assert!(deadline.reserve(Direction::Read, 1).is_err());
    assert_eq!(deadline.snapshot(), before);
    drop(held);
}

#[test]
fn different_pe_services_do_not_share_byte_debt() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let left = Service::new(2, &clock).unwrap();
    let right = Service::new(2, &clock).unwrap();
    let left_ticket = left.reserve(Direction::Read, 9).unwrap();
    let right_ticket = right.reserve(Direction::Write, 1).unwrap();
    assert_eq!(left_ticket.deadline_tick(), 5);
    assert_eq!(right_ticket.deadline_tick(), 1);
    assert_eq!(right.snapshot().admitted_read_bytes, 0);
    assert_eq!(left.snapshot().admitted_write_bytes, 0);
    drop((left_ticket, right_ticket));
}

#[test]
fn local_range_union_deduplicates_bytes_without_charging_padding() {
    assert_eq!(
        super::union(vec![80..84, 80..84, 82..84, 88..89]),
        vec![80..84, 88..89]
    );
    assert_eq!(super::union(vec![80..84, 84..88]), vec![80..88]);
}
