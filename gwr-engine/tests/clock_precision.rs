// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::test_helpers::start_test;
use gwr_engine::time::clock::{ClockTick, Phase, phase};
use gwr_track::Tracker;
use gwr_track::test_helpers::TestTracker;

const MAX_SAFE_TICKS: u64 = (1 << 51) - 1;
const TEN_GHZ_HORIZON_NS: f64 = 225_179_981_368_524.7;

fn assert_precision_panic(action: impl FnOnce()) {
    let panic = catch_unwind(AssertUnwindSafe(action)).expect_err("expected clock precision panic");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("panic must explain the precision limit");
    assert!(message.contains("precision"), "{message}");
}

#[test]
fn boundary_end_is_allowed_but_the_next_tick_panics_before_mutation() {
    let recorded = Rc::new(TestTracker::new(0, log::Level::Trace));
    let tracker: Tracker = recorded.clone();
    let mut engine = Engine::new(&tracker);
    let clock = engine.clock_ghz(10.0);
    let task_clock = clock.clone();
    engine.spawn(async move {
        task_clock.wait_ticks(MAX_SAFE_TICKS)?.await;
        task_clock.wait_phase(phase::END)?.await;
        Ok(())
    });
    engine.run().unwrap();
    let boundary_end = ClockTick::new()
        .set_tick(MAX_SAFE_TICKS)
        .set_phase(Phase::User(phase::END));
    assert_eq!(clock.tick_now(), boundary_end);

    let checkpoint = Rc::new(Cell::new(0));
    let task_checkpoint = checkpoint.clone();
    let task_clock = clock.clone();
    engine.spawn(async move {
        let delay = task_clock.wait_ticks(1)?;
        task_checkpoint.set(1);
        delay.await;
        task_checkpoint.set(2);
        Ok(())
    });
    assert_precision_panic(|| {
        let _ = engine.run();
    });
    assert_eq!(
        checkpoint.get(),
        1,
        "creation succeeds; the event must not resume"
    );
    assert_eq!(engine.time_now_ns(), TEN_GHZ_HORIZON_NS);
    assert_eq!(clock.tick_now(), boundary_end);
    assert_eq!(
        recorded
            .events()
            .iter()
            .filter(|s| s.contains(": set time "))
            .count(),
        1
    );
}

#[test]
fn idle_fast_clock_limits_events_on_an_active_slower_clock() {
    let mut engine = start_test(file!());
    let slow = engine.default_clock();
    drop(engine.clock_ghz(10.0));
    engine.spawn(async move {
        slow.wait_ticks(MAX_SAFE_TICKS / 10 + 1)?.await;
        panic!("the slower clock must not advance beyond the idle clock's limit");
    });
    assert_precision_panic(|| {
        let _ = engine.run();
    });
    assert_eq!(engine.time_now_ns(), 0.0);
}

#[test]
fn clock_registration_panics_if_its_horizon_is_already_past() {
    let mut engine = start_test(file!());
    let slow = engine.default_clock();
    engine.spawn(async move {
        slow.wait_ticks(MAX_SAFE_TICKS / 2)?.await;
        Ok(())
    });
    engine.run().unwrap();
    assert_precision_panic(|| drop(engine.clock_ghz(10.0)));
    assert_eq!(engine.time_now_ns(), (MAX_SAFE_TICKS / 2) as f64);
}

#[test]
fn background_future_beyond_the_horizon_does_not_prevent_termination() {
    let mut engine = start_test(file!());
    let clock = engine.clock_ghz(10.0);
    engine.spawn(async move {
        clock.wait_ticks_or_exit(MAX_SAFE_TICKS + 1)?.await;
        panic!("background future must not resume after simulation termination");
    });
    engine.run().unwrap();
    assert_eq!(engine.time_now_ns(), 0.0);
}

#[test]
fn nonfinite_selected_deadline_reports_precision_even_with_an_idle_clock() {
    let mut engine = start_test(file!());
    drop(engine.default_clock());
    // Its supported horizon is finite, but this later deadline converts to
    // infinity.
    let slow = engine.clock_mhz(2e-290);
    engine.spawn(async move {
        slow.wait_ticks(u64::MAX)?.await;
        Ok(())
    });
    assert_precision_panic(|| {
        let _ = engine.run();
    });
    assert_eq!(engine.time_now_ns(), 0.0);
}

#[test]
fn unsupported_clock_frequencies_panic_at_registration() {
    for frequency in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX, 1e-300] {
        let mut engine = start_test(file!());
        assert_precision_panic(|| drop(engine.clock_mhz(frequency)));
        assert_eq!(engine.time_now_ns(), 0.0);
    }
}
