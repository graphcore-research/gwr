// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Compile actual private service source into this isolated native-clock
//! harness.
#[path = "../src/processing_element/boundary_payload.rs"]
mod boundary_payload;
use boundary_payload::{BoundaryPayloadDirection as Direction, BoundaryPayloadService};
#[test]
#[expect(
    clippy::many_single_char_names,
    reason = "Use conventional matrix dimensions and operand names"
)]
fn exact_combined_payload_debt_has_fractional_bursts_and_no_idle_credit() {
    let mut engine = gwr_engine::engine::Engine::default();
    let clock = engine.default_clock();
    let service = BoundaryPayloadService::new(4, &clock).unwrap();
    let a = service.reserve(Direction::Write, 3).unwrap();
    let b = service.reserve(Direction::Read, 3).unwrap();
    assert_eq!((a.deadline_tick(), b.deadline_tick()), (1, 2));
    let s = service.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        a.complete().await?;
        b.complete().await?;
        c.wait_ticks(98)?.await;
        let late = s.reserve(Direction::Write, 1)?;
        assert_eq!(late.deadline_tick(), 101);
        late.complete().await?;
        Ok(())
    });
    engine.run().unwrap();
    let v = service.snapshot();
    assert_eq!((v.admitted_read_bytes, v.admitted_write_bytes), (3, 4));
    assert_eq!((v.completed_read_bytes, v.completed_write_bytes), (3, 4));
    assert_eq!((v.canceled_read_bytes, v.canceled_write_bytes), (0, 0));
}
#[test]
fn canceled_wait_releases_ownership_but_retains_reserved_debt() {
    let mut engine = gwr_engine::engine::Engine::default();
    let clock = engine.default_clock();
    let service = BoundaryPayloadService::new(4, &clock).unwrap();
    let canceled = service.reserve(Direction::Read, 8).unwrap();
    // Exercise cancellation after the real Clock wait has been polled Pending.
    let deadline = canceled.deadline_tick();
    let mut pending = Box::pin(canceled.complete());
    let waker = futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    assert!(std::future::Future::poll(pending.as_mut(), &mut context).is_pending());
    assert!(clock.tick_now().tick() < deadline);
    drop(pending);
    let next = service.reserve(Direction::Write, 1).unwrap();
    assert_eq!(next.deadline_tick(), 3);
    engine.spawner().spawn(async move {
        next.complete().await?;
        Ok(())
    });
    engine.run().unwrap();
    let s = service.snapshot();
    assert_eq!(
        (
            s.admitted_read_bytes,
            s.canceled_read_bytes,
            s.completed_read_bytes
        ),
        (8, 8, 0)
    );
    assert_eq!(
        (
            s.admitted_write_bytes,
            s.completed_write_bytes,
            s.canceled_write_bytes
        ),
        (1, 1, 0)
    );
}
