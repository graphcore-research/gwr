// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This module represents the time during a simulation.
//!
//! Time is made up of a cycle count and a phase.

use std::rc::Rc;

use gwr_track::entity::Entity;
use gwr_track::set_time;

use super::clock::Clock;
use crate::time::clock::TaskWaker;

/// The overall owner of time within a simulation.
///
/// Contains all Clocks and the current simulation time in ns.
#[derive(Clone)]
pub struct SimTime {
    entity: Rc<Entity>,

    current_ns: f64,

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
            current_ns: 0.0,
            clocks: Vec::new(),
        }
    }

    pub fn get_clock(&mut self, freq_mhz: f64) -> Clock {
        for clock in &self.clocks {
            if clock.freq_mhz() == freq_mhz {
                return clock.clone();
            }
        }
        let clock = Clock::new(freq_mhz);
        self.clocks.push(clock.clone());
        clock
    }

    /// Choose the clock with the next time and return the associated Waker.
    pub fn advance_time(&mut self) -> Option<Vec<TaskWaker>> {
        if let Some(next_clock) = self.clocks.iter().min_by(|a, b| a.cmp(b)) {
            if let Some(clock_time) = next_clock.shared_state.waiting_times.borrow_mut().pop() {
                let next_ns = next_clock.to_ns(&clock_time);
                if self.current_ns != next_ns {
                    set_time!(self.entity ; next_ns);
                    self.current_ns = next_ns;
                }
                next_clock.advance_time(clock_time);
                next_clock.shared_state.waiting.borrow_mut().pop()
            } else {
                None
            }
        } else {
            None
        }
    }

    #[must_use]
    pub fn time_now_ns(&self) -> f64 {
        self.current_ns
    }

    /// The simulation can exit if all scheduled tasks can exit.
    #[must_use]
    pub fn can_exit(&self) -> bool {
        for clock in &self.clocks {
            for waiting in clock.shared_state.waiting.borrow().iter() {
                for task_waker in waiting {
                    if !task_waker.can_exit {
                        // Found one task that must be completed
                        return false;
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use futures::task::noop_waker;
    use gwr_track::entity::toplevel;
    use gwr_track::test_helpers::create_tracker;

    use super::*;

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
    fn advance_time_returns_none_without_waiters() {
        let tracker = create_tracker(file!());
        let top = toplevel(&tracker, "top");

        let mut time = SimTime::new(&top);
        assert!(time.advance_time().is_none());

        let _clock = time.get_clock(1000.0);
        assert!(time.advance_time().is_none());
    }

    #[test]
    fn advance_time_handles_equal_times_on_different_clocks() {
        let tracker = create_tracker(file!());
        let top = toplevel(&tracker, "top");

        let mut time = SimTime::new(&top);
        let clock_1ghz = time.get_clock(1000.0);
        let clock_2ghz = time.get_clock(2000.0);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut delays = [
            clock_1ghz.wait_ticks(1),
            clock_1ghz.wait_ticks(2),
            clock_2ghz.wait_ticks(2),
            clock_2ghz.wait_ticks(4),
        ];

        for delay in &mut delays {
            assert_eq!(Pin::new(delay).poll(&mut cx), Poll::Pending);
        }

        for expected_ns in [1.0, 1.0, 2.0, 2.0] {
            let wakers = time.advance_time().unwrap();

            assert_eq!(wakers.len(), 1);
            assert_eq!(time.time_now_ns(), expected_ns);
        }

        assert_eq!(clock_1ghz.tick_now().tick(), 2);
        assert_eq!(clock_2ghz.tick_now().tick(), 4);
        assert!(time.advance_time().is_none());
    }
}
