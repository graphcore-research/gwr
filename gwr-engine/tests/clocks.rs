// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::{FutureExt, select};
use gwr_engine::events::once::Once;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::test_helpers::start_test;
use gwr_engine::time::clock::phase;
use gwr_engine::traits::{Event, Resolve, Resolver};

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
            clk1.wait_ticks(1)?.await;
            values.borrow_mut().push((1, clk1.time_now_ns()));
        }
        Ok(())
    });

    let values = all_values.clone();
    engine.spawn(async move {
        for _ in 0..5 {
            clk2.wait_ticks(1)?.await;
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
                clk.wait_ticks(1)?.await;
            }
            Ok(())
        });
    }

    {
        let clk = engine.default_clock();
        engine.spawn(async move {
            for _ in 0..50 {
                clk.wait_ticks_or_exit(10)?.await;
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
            clock.wait_ticks(1)?.await;
            state.queue(&clock, 1, state.clone());
            Ok(())
        });
    }

    {
        let clock = engine.default_clock();
        let state = state.clone();
        engine.spawn(async move {
            clock.wait_ticks(1)?.await;
            assert_eq!(state.value.get(), 0);
            clock.wait_ticks(1)?.await;
            assert_eq!(state.value.get(), 1);
            Ok(())
        });
    }

    engine.run().unwrap();
}

#[test]
fn clock_frequency_helpers_agree() {
    let mut engine = start_test("clocks");

    let clock_mhz = engine.clock_mhz(1.0);
    let clock_khz = engine.clock_khz(1000.0);
    let clock_hz = engine.clock_hz(1_000_000.0);
    let clock_ghz = engine.clock_ghz(0.001);

    assert_eq!(clock_mhz.freq_mhz(), clock_hz.freq_mhz());
    assert_eq!(clock_mhz.freq_mhz(), clock_khz.freq_mhz());
    assert_eq!(clock_mhz.freq_mhz(), clock_ghz.freq_mhz());
}

#[test]
fn cancelled_wait_ticks_does_not_leave_stale_schedule() {
    let mut engine = start_test("clocks");

    {
        let clock = engine.default_clock();
        engine.spawn(async move {
            let mut short_wait = clock.wait_ticks(5)?.fuse();
            let mut long_wait = clock.wait_ticks(50)?.fuse();

            select! {
                () = short_wait => {}
                () = long_wait => panic!("long wait should have been cancelled"),
            }

            clock.wait_ticks(5)?.await;
            assert_eq!(clock.time_now_ns(), 10.0);
            Ok(())
        });
    }

    engine.run().unwrap();
    assert_eq!(engine.time_now_ns(), 10.0);
}

/// Test waiting ticks after an event from another clock, including events
/// between ticks.
#[test]
fn wait_ticks_after_event_from_another_clock() {
    let fast_freq_mhz = 1000.0;
    let slow_freq_mhz = 500.0;

    for (fast_clock_ticks, slow_clock_ticks, expected_time_ns) in &[
        // Advance to a slow clock tick and then trigger the event.
        (10, 1, 12.0),
        // Advance to a point between slow clock ticks and then trigger the event.
        (9, 1, 10.0),
        (9, 2, 12.0),
    ] {
        let mut engine = start_test("clocks");
        let fast = engine.clock_mhz(fast_freq_mhz);
        let slow = engine.clock_mhz(slow_freq_mhz);
        let once = Once::default();

        {
            let once = once.clone();
            engine.spawn(async move {
                fast.wait_ticks(*fast_clock_ticks)?.await;
                once.notify()?;
                Ok(())
            });
        }

        engine.spawn(async move {
            once.listen().await;
            slow.wait_ticks(*slow_clock_ticks)?.await;
            Ok(())
        });

        engine.run().unwrap();
        assert_eq!(engine.time_now_ns(), *expected_time_ns);
    }
}

/// Test waiting for a phase after an event from another clock, including events
/// between ticks.
#[test]
fn wait_phase_after_event_from_another_clock() {
    for requested_phase in [phase::BEGIN, phase::END] {
        let mut engine = start_test("clocks");
        let fast = engine.clock_mhz(1000.0);
        let slow = engine.clock_mhz(500.0);
        let event = Once::default();
        let observed = Rc::new(Cell::new(None));

        {
            let event = event.clone();
            engine.spawn(async move {
                fast.wait_ticks(9)?.await;
                event.notify()?;
                Ok(())
            });
        }

        {
            let observed = observed.clone();
            engine.spawn(async move {
                event.listen().await;
                slow.wait_phase(requested_phase)?.await;

                let now = slow.tick_now();
                observed.set(Some((now.tick(), now.phase())));
                Ok(())
            });
        }

        engine.run().unwrap();

        assert_eq!(observed.get(), Some((5, requested_phase)));
        assert_eq!(engine.time_now_ns(), 10.0);
    }
}

/// Test to ensure that resolves are handled when time is advanced by via
/// another clock.
#[test]
fn pending_updates_resolve_after_event_from_another_clock() {
    let fast_freq_mhz = 1000.0;
    let slow_freq_mhz = 500.0;

    for (fast_ticks_before, fast_ticks_after, expected_time_ns) in
        &[(0, 1, 1.0), (0, 2, 2.0), (1, 2, 3.0)]
    {
        let mut engine = start_test("clocks");
        let fast = engine.clock_mhz(fast_freq_mhz);
        let slow = engine.clock_mhz(slow_freq_mhz);
        let event = Repeated::default();
        let state = Rc::new(PendingUpdate::new(0));
        let receiver_completed = Rc::new(Cell::new(false));

        // Ensure the first event listener is created before any notifications
        // are sent.
        let first_event = event.listen();

        {
            let event = event.clone();
            engine.spawn(async move {
                fast.wait_ticks(*fast_ticks_before)?.await;
                event.notify();

                fast.wait_ticks(*fast_ticks_after)?.await;
                event.notify();
                Ok(())
            });
        }

        {
            let receiver_completed = receiver_completed.clone();
            engine.spawn(async move {
                first_event.await;
                state.queue(&slow, 1, state.clone());
                assert_eq!(state.value.get(), 0);

                event.listen().await;
                assert_eq!(state.value.get(), 1);

                receiver_completed.set(true);
                Ok(())
            });
        }

        engine.run().unwrap();
        assert_eq!(engine.time_now_ns(), *expected_time_ns);
        assert!(receiver_completed.get());
    }
}

/// Test that pending updates are resolved before the simulation exits.
#[test]
fn pending_resolves_complete_before_simulation_exits() {
    let mut engine = start_test("clocks");
    let state = Rc::new(PendingUpdate::new(0));

    {
        let clock = engine.default_clock();
        let state = state.clone();
        engine.spawn(async move {
            state.queue(&clock, 1, state.clone());
            assert_eq!(state.value.get(), 0);
            Ok(())
        });
    }

    engine.run().unwrap();

    assert_eq!(state.value.get(), 1);
    assert_eq!(engine.time_now_ns(), 0.0);
}

/// Test that pending updates are resolved after the end phase.
#[test]
fn pending_updates_resolve_after_end_phase() {
    use gwr_engine::time::clock::phase;

    let mut engine = start_test("clocks");
    let state = Rc::new(PendingUpdate::new(0));
    let value_at_end = Rc::new(Cell::new(None));

    {
        let clock = engine.default_clock();
        let state = state.clone();
        let value_at_end = value_at_end.clone();

        engine.spawn(async move {
            clock.wait_ticks(1)?.await;
            state.queue(&clock, 1, state.clone());

            clock.wait_phase(phase::END)?.await;
            assert_eq!(clock.tick_now().phase(), phase::END);
            value_at_end.set(Some(state.value.get()));
            Ok(())
        });
    }

    engine.run().unwrap();

    assert_eq!(value_at_end.get(), Some(0));
    assert_eq!(state.value.get(), 1);
    assert_eq!(engine.time_now_ns(), 1.0);
}

#[derive(Default)]
struct CountingResolve {
    calls: Cell<u64>,
}

impl Resolve for CountingResolve {
    fn resolve(&self) {
        self.calls.set(self.calls.get() + 1);
    }
}

/// Test that confirms resolves run only once per registration.
#[test]
fn resolves_run_once_per_registration() {
    let mut engine = start_test("clocks");
    let first = Rc::new(CountingResolve::default());
    let second = Rc::new(CountingResolve::default());

    {
        let clock = engine.default_clock();
        let first = first.clone();
        let second = second.clone();

        engine.spawn(async move {
            clock.add_resolve(first.clone());
            clock.add_resolve(second.clone());

            clock.wait_ticks(1)?.await;
            assert_eq!((first.calls.get(), second.calls.get()), (1, 1));

            // Register another update in tick 1.
            clock.add_resolve(first.clone());
            Ok(())
        });
    }

    engine.run().unwrap();

    assert_eq!((first.calls.get(), second.calls.get()), (2, 1));
    assert_eq!(engine.time_now_ns(), 1.0);
}

/// Test that clocks synchronize correctly at large tick counts
#[test]
fn synchronizing_at_large_tick_counts() {
    let mut engine = start_test("exact_clock_boundaries");
    let fast = engine.clock_mhz(2000.0);
    let slow = engine.clock_mhz(1000.0);

    engine.spawn(async move {
        // Use the fast clock to advance until the slow clock ticks alias in an
        // f64 nanosecond counter
        fast.wait_ticks(1 << 54)?.await;

        // The slow clock should synchronize to the same phase in a coincident
        // tick
        assert_eq!(slow.tick_now().tick(), 1 << 53);
        assert_eq!(slow.tick_now().phase(), phase::BEGIN);
        Ok(())
    });

    engine.run().unwrap();
}

/// Test that clocks synchronize between ticks correctly at large tick counts
#[test]
fn synchronizing_between_ticks_at_large_tick_counts() {
    let mut engine = start_test("exact_clock_phase_reset");
    let fast = engine.clock_mhz(2000.0);
    let slow = engine.clock_mhz(1000.0);

    engine.spawn(async move {
        fast.wait_ticks(1 << 53)?.await;
        fast.wait_phase(2)?.await;

        // The slow clock should synchronize to the same phase in a coincident
        // tick
        assert_eq!(slow.tick_now().tick(), 1 << 52);
        assert_eq!(slow.tick_now().phase(), 2);

        // Now wait for an earlier phase in the next tick
        fast.next_tick_and_phase(1)?.await;
        assert_eq!(fast.tick_now().tick(), (1 << 53) + 1);
        assert_eq!(fast.tick_now().phase(), 1);

        // The slow clock should now be between ticks
        assert_eq!(slow.tick_now().tick(), 1 << 52);
        assert_eq!(slow.tick_now().phase(), phase::NONE);

        Ok(())
    });

    engine.run().unwrap();
}

/// Test that events are handled in the correct order when f64 nanosecond times
/// coincide
#[test]
fn event_order_is_correct_when_f64_ns_timestamps_coincide() {
    let mut engine = start_test("exact_clock_order");
    let fast = engine.clock_mhz(2000.0);
    let slow = engine.clock_mhz(1000.0);

    let order = Rc::new(RefCell::new(Vec::new()));

    let observed = order.clone();
    engine.spawn(async move {
        fast.wait_ticks((1 << 54) + 1)?.await;
        observed.borrow_mut().push("fast");
        Ok(())
    });

    let observed = order.clone();
    engine.spawn(async move {
        slow.wait_ticks(1 << 53)?.await;
        observed.borrow_mut().push("slow");
        Ok(())
    });

    engine.run().unwrap();

    assert_eq!(*order.borrow(), ["slow", "fast"]);
}

/// Test that ns time is monotonic even when raw conversion to ns using
///   (tick as f64 / freq_mhz) * 1000.0
/// produces different values for coincident ticks of different clocks.
#[test]
fn time_is_monotonic_when_clocks_could_report_different_ns() {
    let mut engine = start_test("exact_clock_reporting");
    let fast = engine.clock_mhz(9000.0);
    let slow = engine.clock_mhz(1000.0);

    engine.spawn(async move {
        let tick = (1_u64 << 50) + 3;
        fast.wait_ticks(9 * tick)?.await;

        // The raw conversion ((9 * tick) as f64 / 9000.0) * 1000.0
        // produces tick + 0.25 ns, or 1125899906842627.25 ns.
        let before = fast.time_now_ns();

        slow.wait_phase(1)?.await;

        // The raw conversion (tick as f64 / 1000.0) * 1000.0 would
        // produce tick ns, or 1125899906842627.0 ns. Preserve the previous
        // report at this exact tie so reported time does not go backwards.
        let after = slow.time_now_ns();

        assert!(
            after >= before,
            "reported ns moved backwards: {before:?} -> {after:?}"
        );

        Ok(())
    });

    engine.run().unwrap();
}
