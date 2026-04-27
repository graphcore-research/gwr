// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

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
            values.borrow_mut().push((1, clk1.time_now_ns()));
        }
        Ok(())
    });

    let values = all_values.clone();
    engine.spawn(async move {
        for _ in 0..5 {
            clk2.wait_ticks(1).await;
            values.borrow_mut().push((2, clk2.time_now_ns()));
        }
        Ok(())
    });

    engine.run().unwrap();

    let ns1 = 1000.0 / mhz1;
    let ns2 = 1000.0 / mhz2;
    assert_eq!(
        vec![
            (2, 1.0 * ns2),
            (1, 1.0 * ns1),
            (2, 2.0 * ns2),
            (2, 3.0 * ns2),
            (1, 2.0 * ns1),
            (2, 4.0 * ns2),
            (2, 5.0 * ns2),
            (1, 3.0 * ns1),
            (1, 4.0 * ns1),
            (1, 5.0 * ns1),
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
    assert_eq!(engine.time_now_ns(), 5.0);
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

/// Test that checks that if two tasks are resumed in the same tick that updates
/// queued by one are resolved before the next runs.
/// Note: this relies on tasks being resumed in spawn order within a tick.
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
            assert_eq!(state.value.get(), 1);
            Ok(())
        });
    }

    engine.run().unwrap();
}
