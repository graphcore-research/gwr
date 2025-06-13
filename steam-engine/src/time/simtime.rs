// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This module represents the time during a simulation.
//!
//! Time is made up of a cycle count and a phase.

use std::sync::Arc;

use steam_track::entity::Entity;
use steam_track::set_time;

use super::clock::Clock;
use crate::time::clock::TaskWaker;

/// The overall owner of time within a simulation.
///
/// Contains all Clocks and the current simulation time in ns.
#[derive(Clone)]
pub struct SimTime {
    pub entity: Arc<Entity>,

    current_ns: f64,

    /// Clocks are auto-created as required and kept in a HashMap.
    ///
    /// They are hashed using a `u64` which is done in `Hz` so there is a chance
    /// that a certain clock f
    clocks: Vec<Clock>,
}

impl SimTime {
    #[must_use]
    pub fn new(parent: &Arc<Entity>) -> Self {
        Self {
            entity: Arc::new(Entity::new(parent, "time")),
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
                next_clock.shared_state.waiting.borrow_mut().pop()
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Move all clocks to their next tick after the specified time.
    pub fn advance_all_clocks_to(&mut self, time_ns: f64) {
        for clock in &self.clocks {
            clock.advance_to(time_ns);
        }
        set_time!(self.entity ; time_ns);
        self.current_ns = time_ns;
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
    use steam_track::entity::toplevel;

    use super::*;
    use crate::test_helpers::create_tracker;

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
}
