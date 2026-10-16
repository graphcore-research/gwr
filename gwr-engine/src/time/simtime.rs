// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This module represents the time during a simulation.
//!
//! Time is made up of a tick count and a phase.

use std::cell::Cell;
use std::rc::Rc;

use gwr_track::entity::Entity;
use gwr_track::set_time;

use super::clock::{Clock, phase};
use crate::time::clock::SimInstant;

/// The overall owner of time within a simulation.
///
/// Contains all Clocks and the current simulation time in ns.
pub struct SimTime {
    entity: Rc<Entity>,

    now: Rc<Cell<SimInstant>>,

    /// Clocks are auto-created as required and kept in a HashMap.
    ///
    /// They are hashed using a `u64` which is done in `Hz` so there is a chance
    /// that a certain clock f
    clocks: Vec<Clock>,
}

impl SimTime {
    #[must_use]
    pub fn new(parent: &Rc<Entity>) -> Self {
        Self {
            entity: Rc::new(Entity::new(parent, "time")),
            now: Rc::new(Cell::new(SimInstant {
                ns: 0.0,
                phase: phase::BEGIN,
            })),
            clocks: Vec::new(),
        }
    }

    pub fn get_clock(&mut self, freq_mhz: f64) -> Clock {
        for clock in &self.clocks {
            if clock.freq_mhz() == freq_mhz {
                return clock.clone();
            }
        }
        let clock = Clock::new(freq_mhz, self.now.clone());
        self.clocks.push(clock.clone());
        clock
    }

    /// If any clocks have required events outstanding, choose the clock with
    /// the next event and advance the simulation to that point.
    pub fn try_advance(&mut self) -> bool {
        let can_exit = self.clocks.iter().all(|clock| clock.can_exit());
        if can_exit {
            return false;
        }

        let next_clock = self
            .clocks
            .iter()
            .min_by(|a, b| a.cmp_next_event(b))
            .unwrap();
        assert!(next_clock.has_scheduled_events());

        // Update sim time and the tracker, and then bring the clock up to date

        let next_instant = SimInstant {
            ns: next_clock.time_of_next(),
            phase: next_clock.phase_of_next(),
        };
        assert!(next_instant.ns >= self.now.get().ns);
        if next_instant.ns == self.now.get().ns {
            assert!(next_instant.phase >= self.now.get().phase);
        } else {
            set_time!(self.entity ; next_instant.ns);
        }
        self.now.set(next_instant);

        next_clock.advance_to_event();

        true
    }

    #[must_use]
    pub fn time_now_ns(&self) -> f64 {
        self.now.get().ns
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    use gwr_track::entity::toplevel;
    use gwr_track::test_helpers::create_tracker;

    use super::*;

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn clock_created_once() {
        let tracker = create_tracker(file!());
        let top = toplevel(&tracker, "top");

        let mut time = SimTime::new(&top);
        let _clk1 = time.get_clock(1000.0);
        assert_eq!(time.clocks.len(), 1);

        let _clk2 = time.get_clock(1000.0);
        assert_eq!(time.clocks.len(), 1);
    }

    #[test]
    fn create_different_clocks() {
        let tracker = create_tracker(file!());
        let top = toplevel(&tracker, "top");

        let mut time = SimTime::new(&top);
        let _clk1 = time.get_clock(1000.0);
        assert_eq!(time.clocks.len(), 1);

        let _clk2 = time.get_clock(1800.0);
        assert_eq!(time.clocks.len(), 2);
    }

    #[test]
    fn exits_without_waiters() {
        let tracker = create_tracker(file!());
        let top = toplevel(&tracker, "top");

        let mut time = SimTime::new(&top);
        assert!(!time.try_advance());
        assert_eq!(time.time_now_ns(), 0.0);

        let _clock = time.get_clock(1000.0);
        assert!(!time.try_advance());
        assert_eq!(time.time_now_ns(), 0.0);
    }

    #[test]
    fn advance_time_handles_equal_times_on_different_clocks() {
        let tracker = create_tracker(file!());
        let top = toplevel(&tracker, "top");

        let mut time = SimTime::new(&top);
        let clock_1ghz = time.get_clock(1000.0);
        let clock_2ghz = time.get_clock(2000.0);
        let wakes = Arc::new(WakeCounter::default());
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);

        let mut delays = [
            clock_1ghz.wait_ticks(1).unwrap(),
            clock_1ghz.wait_ticks(2).unwrap(),
            clock_2ghz.wait_ticks(2).unwrap(),
            clock_2ghz.wait_ticks(4).unwrap(),
        ];

        for delay in &mut delays {
            assert_eq!(Pin::new(delay).poll(&mut cx), Poll::Pending);
        }

        for (index, expected_ns) in [1.0, 1.0, 2.0, 2.0].into_iter().enumerate() {
            assert!(time.try_advance());

            assert_eq!(wakes.0.load(Ordering::Relaxed), index + 1);
            assert_eq!(time.time_now_ns(), expected_ns);
        }

        assert_eq!(clock_1ghz.tick_now().tick(), 2);
        assert_eq!(clock_2ghz.tick_now().tick(), 4);
        assert!(!time.try_advance());
        assert_eq!(time.time_now_ns(), 2.0);
        for delay in &mut delays {
            assert_eq!(Pin::new(delay).poll(&mut cx), Poll::Ready(()));
        }
    }

    #[test]
    fn advances_in_deadline_order_regardless_of_insertion_order_or_removals() {
        let tracker = create_tracker(file!());
        let top = toplevel(&tracker, "top");
        let mut time = SimTime::new(&top);
        let clock = time.get_clock(1000.0);
        let wakes = Arc::new(WakeCounter::default());
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);
        let mut delays = Vec::new();

        // Exercise inserts before, between and after existing deadlines,
        // including multiple waiters at the earliest and latest deadlines.
        for ticks in [31, 1, 16, 7, 31, 1, 8, 2, 30, 9] {
            let mut delay = clock.wait_ticks(ticks).unwrap();
            assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Pending);
            delays.push(delay);
        }
        drop(delays.remove(2)); // Remove the only waiter at tick 16.
        drop(delays.remove(0)); // Remove one waiter at tick 31.

        let mut expected_wakes = 0;
        for (ticks, count) in [(1, 2), (2, 1), (7, 1), (8, 1), (9, 1), (30, 1), (31, 1)] {
            assert!(time.try_advance());
            assert_eq!(clock.tick_now().tick(), ticks);
            expected_wakes += count;
            assert_eq!(wakes.0.load(Ordering::Relaxed), expected_wakes);
        }
        assert!(!time.try_advance());
    }
}
