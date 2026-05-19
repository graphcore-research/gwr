// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use futures::{FutureExt, select};
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::{Resolve, Resolver};

/// Test that runs different clocks that add to a shared vector and then checks
/// that everything has been done in the correct order and at the right times.
#[test]
fn dual_clock() {
    let mut engine = start_test("clocks");

    let mhz1 = 1000.0;
    let mhz2 = 1800.0;

    let clk1 = engine.clock_mhz(mhz1);
    let clk2 = engine.clock_mhz(mhz2);

    let all_values = Rc::new(RefCell::new(Vec::new()));

    let values = all_values.clone();
    engine.spawn(async move {
        for _ in 0..5 {
            clk1.wait_ticks(1).await;
            values.borrow_mut().push((1, clk1.time_now()));
        }
        Ok(())
    });

    let values = all_values.clone();
    engine.spawn(async move {
        for _ in 0..5 {
            clk2.wait_ticks(1).await;
            values.borrow_mut().push((2, clk2.time_now()));
        }
        Ok(())
    });

    engine.run().unwrap();

    assert_eq!(
        vec![
            (2, Duration::from_secs_f64(1.0 / (mhz2 * 1e6))),
            (1, Duration::from_secs_f64(1.0 / (mhz1 * 1e6))),
            (2, Duration::from_secs_f64(2.0 / (mhz2 * 1e6))),
            (2, Duration::from_secs_f64(3.0 / (mhz2 * 1e6))),
            (1, Duration::from_secs_f64(2.0 / (mhz1 * 1e6))),
            (2, Duration::from_secs_f64(4.0 / (mhz2 * 1e6))),
            (2, Duration::from_secs_f64(5.0 / (mhz2 * 1e6))),
            (1, Duration::from_secs_f64(3.0 / (mhz1 * 1e6))),
            (1, Duration::from_secs_f64(4.0 / (mhz1 * 1e6))),
            (1, Duration::from_secs_f64(5.0 / (mhz1 * 1e6))),
        ],
        *all_values.borrow()
    );
}

// Ensure that a task that calls `wait_ticks_or_exit` doesn't stop a simulation
// from terminating.
#[test]
fn wait_ticks_or_exit() {
    let mut engine = start_test("clocks");

    {
        let clk = engine.default_clock();
        engine.spawn(async move {
            for _ in 0..5 {
                clk.wait_ticks(1).await;
            }
            Ok(())
        });
    }

    {
        let clk = engine.default_clock();
        engine.spawn(async move {
            for _ in 0..50 {
                clk.wait_ticks_or_exit(10).await;
            }
            Ok(())
        });
    }

    engine.run().unwrap();

    // Simulation should have finished when the first loop completed
    assert_eq!(engine.time_now(), Duration::from_nanos(5));
}

struct PendingUpdate {
    value: Cell<u64>,
    pending: Cell<Option<u64>>,
}

impl PendingUpdate {
    fn new(value: u64) -> Self {
        Self {
            value: Cell::new(value),
            pending: Cell::new(None),
        }
    }

    fn queue(&self, resolver: &impl Resolver, value: u64, state: Rc<Self>) {
        self.pending.set(Some(value));
        resolver.add_resolve(state);
    }
}

impl Resolve for PendingUpdate {
    fn resolve(&self) {
        if let Some(value) = self.pending.take() {
            self.value.set(value);
        }
    }
}

/// Test that checks that if two tasks are resumed in the same clock tick,
/// that updates queued by one are only visible in the next clock tick.
#[test]
fn same_tick_waits_resolve_queued_updates_between_waiters() {
    let mut engine = start_test("clocks");
    let state = Rc::new(PendingUpdate::new(0));

    {
        let clock = engine.default_clock();
        let state = state.clone();
        engine.spawn(async move {
            clock.wait_ticks(1).await;
            state.queue(&clock, 1, state.clone());
            Ok(())
        });
    }

    {
        let clock = engine.default_clock();
        let state = state.clone();
        engine.spawn(async move {
            clock.wait_ticks(1).await;
            assert_eq!(state.value.get(), 0);
            clock.wait_ticks(1).await;
            assert_eq!(state.value.get(), 1);
            Ok(())
        });
    }

    engine.run().unwrap();
}

#[test]
fn clock_hz_matches_clock_mhz() {
    let mut engine = start_test("clocks");

    let clock_mhz = engine.clock_mhz(1.0);
    let clock_khz = engine.clock_mhz(1.0);
    let clock_hz = engine.clock_hz(1_000_000.0);

    assert_eq!(clock_mhz.freq_mhz(), clock_hz.freq_mhz());
    assert_eq!(clock_mhz.freq_mhz(), clock_khz.freq_mhz());
    assert!(Rc::ptr_eq(&clock_mhz.shared_state, &clock_hz.shared_state));
    assert!(Rc::ptr_eq(&clock_mhz.shared_state, &clock_khz.shared_state));
}

#[test]
fn cancelled_wait_ticks_does_not_leave_stale_schedule() {
    let mut engine = start_test("clocks");

    {
        let clock = engine.default_clock();
        engine.spawn(async move {
            let mut short_wait = clock.wait_ticks(5).fuse();
            let mut long_wait = clock.wait_ticks(50).fuse();

            select! {
                () = short_wait => {}
                () = long_wait => panic!("long wait should have been cancelled"),
            }

            clock.wait_ticks(5).await;
            assert_eq!(clock.time_now(), Duration::from_nanos(10));
            Ok(())
        });
    }

    engine.run().unwrap();
    assert_eq!(engine.time_now(), Duration::from_nanos(10));
}
