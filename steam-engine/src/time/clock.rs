// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This module represents the time during a simulation.
//!
//! Time is made up of a cycle count and a phase.

use core::cmp::Ordering;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use crate::traits::{Resolve, Resolver};

/// ClockTick structure for representing a number of Clock ticks and a phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockTick {
    /// Clock ticks.
    tick: u64,

    /// Clock phase.
    phase: u32,
}

impl ClockTick {
    pub fn new() -> Self {
        Self { tick: 0, phase: 0 }
    }

    /// Get the current clock tick.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Get the current clock phase.
    pub fn phase(&self) -> u32 {
        self.phase
    }

    /// Change the default constructor value of `tick`.
    pub fn set_tick(&mut self, tick: u64) -> ClockTick {
        self.tick = tick;
        *self
    }

    /// Change the default constructor value of `phase`.
    pub fn set_phase(&mut self, phase: u32) -> ClockTick {
        self.phase = phase;
        *self
    }
}

impl Default for ClockTick {
    fn default() -> Self {
        Self::new()
    }
}

/// Define the comparison operation for SimTime.
impl Ord for ClockTick {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.tick.cmp(&other.tick) {
            Ordering::Greater => Ordering::Greater,
            Ordering::Less => Ordering::Less,
            Ordering::Equal => self.phase.cmp(&other.phase),
        }
    }
}

impl PartialOrd for ClockTick {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for ClockTick {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{:?}", self.tick, self.phase)
    }
}

#[derive(Clone)]
/// State representing a clock.
pub struct Clock {
    /// Frequency of the clock in MHz.
    /// *Note*: Should never be changed as it is registered at this frequency.
    freq_mhz: f64,

    pub shared_state: Rc<ClockState>,
}

pub struct TaskWaker {
    /// The Waker to use to make a task active again.
    pub waker: Waker,

    /// When a task is scheduled in the future it may be a background task
    /// that will simply run forever in which case it will set `can_exit` to
    /// true.
    pub can_exit: bool,
}

/// Shared state between futures using a Clock and the Clock itself.
pub struct ClockState {
    now: RefCell<ClockTick>,

    /// Queue of futures waiting for the right time.
    pub waiting: RefCell<Vec<Vec<TaskWaker>>>,

    /// Queue of times at which those futures are to be woken. This is kept
    /// sorted by time so that the first entry is the next to be woken.
    pub waiting_times: RefCell<Vec<ClockTick>>,

    /// Registered [`Resolve`] functions.
    pub to_resolve: RefCell<Vec<Rc<dyn Resolve + 'static>>>,
}

impl ClockState {
    fn schedule(&self, schedule_time: ClockTick, cx: &mut Context<'_>, can_exit: bool) {
        let mut waiting_times = self.waiting_times.borrow_mut();
        let mut waiting = self.waiting.borrow_mut();
        if let Some(index) = waiting_times.iter().position(|&x| x == schedule_time) {
            // Time already exists, add this task
            waiting[index].push(TaskWaker {
                waker: cx.waker().clone(),
                can_exit,
            });
        } else {
            // Time not found, insert at the correct location
            match waiting_times.iter().position(|x| *x < schedule_time) {
                Some(index) => {
                    // Insert at an arbitrary index
                    waiting_times.insert(index, schedule_time);
                    waiting.insert(
                        index,
                        vec![TaskWaker {
                            waker: cx.waker().clone(),
                            can_exit,
                        }],
                    );
                }
                None => {
                    // Insert at the head
                    waiting_times.push(schedule_time);
                    waiting.push(vec![TaskWaker {
                        waker: cx.waker().clone(),
                        can_exit,
                    }]);
                }
            };
        }
    }

    fn advance_time(&self, to_time: ClockTick) {
        self.resolve();
        if to_time != *self.now.borrow() {
            assert!(to_time >= *self.now.borrow(), "Time moving backwards");
            *self.now.borrow_mut() = to_time;
        }
    }

    fn resolve(&self) {
        for r in self.to_resolve.borrow_mut().drain(..) {
            r.resolve();
        }
    }
}

impl Clock {
    /// Create a new [Clock] at the specified frequency.
    pub fn new(freq_mhz: f64) -> Self {
        let shared_state = Rc::new(ClockState {
            now: RefCell::new(ClockTick { tick: 0, phase: 0 }),
            waiting: RefCell::new(Vec::new()),
            waiting_times: RefCell::new(Vec::new()),
            to_resolve: RefCell::new(Vec::new()),
        });

        Self {
            freq_mhz,
            shared_state,
        }
    }

    /// Returns the clocks frequency in MHz.
    pub fn freq_mhz(&self) -> f64 {
        self.freq_mhz
    }

    /// Returns the current [ClockTick].
    pub fn tick_now(&self) -> ClockTick {
        *self.shared_state.now.borrow()
    }

    /// Returns the current time in `ns`.
    pub fn time_now_ns(&self) -> f64 {
        let now = *self.shared_state.now.borrow();
        self.to_ns(&now)
    }

    /// Returns the time in `ns` of the next event registered with this clock.
    pub fn time_of_next(&self) -> f64 {
        match self.shared_state.waiting_times.borrow().first() {
            Some(clock_time) => self.to_ns(clock_time),
            None => f64::MAX,
        }
    }

    /// Convert the given [ClockTick] to a time in `ns` for this clock.
    pub fn to_ns(&self, clock_time: &ClockTick) -> f64 {
        clock_time.tick as f64 / self.freq_mhz * 1000.0
    }

    /// Returns a [ClockDelay] future which must be `await`ed to delay the
    /// specified number of ticks.
    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn wait_ticks(&self, ticks: u64) -> ClockDelay {
        let mut until = self.tick_now();
        until.tick += ticks;
        ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            state: ClockDelayState::Pending,
            can_exit: false,
        }
    }

    /// Returns a [ClockDelay] future which must be `await`ed to delay the
    /// specified number of ticks. However, if the remainder of the simulation
    /// completes then this future is allowed to not complete. This allows the
    /// user to create tasks that can run continuously as long as the rest of
    /// the simulation continues to run.
    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn wait_ticks_or_exit(&self, ticks: u64) -> ClockDelay {
        let mut until = self.tick_now();
        until.tick += ticks;
        ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            state: ClockDelayState::Pending,
            can_exit: true,
        }
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn next_tick_and_phase(&self, phase: u32) -> ClockDelay {
        let mut until = self.tick_now();
        until.tick += 1;
        until.phase = phase;
        ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            state: ClockDelayState::Pending,
            can_exit: false,
        }
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn wait_phase(&self, phase: u32) -> ClockDelay {
        let mut until = self.tick_now();
        assert!(phase > until.phase, "Time going backwards");
        until.phase = phase;
        ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            state: ClockDelayState::Pending,
            can_exit: false,
        }
    }

    /// Advance to the next tick after the specified time.
    pub fn advance_to(&self, time_ns: f64) {
        let now_ns = self.time_now_ns();
        assert!(now_ns < time_ns);
        let diff_ns = time_ns - now_ns;
        let ticks = (diff_ns * (self.freq_mhz / 1000.0)).ceil();

        let mut until = self.tick_now();
        until.tick += ticks as u64;

        self.shared_state.advance_time(until);
    }
}

/// The default clocks is simply to use a 1GHz clock so ticks are 1ns.
impl Default for Clock {
    fn default() -> Self {
        Self::new(1000.0)
    }
}

/// The comparison operators for Clocks - use the next pending Waker time.
impl PartialEq for Clock {
    fn eq(&self, other: &Self) -> bool {
        self.time_of_next() == other.time_of_next()
    }
}
impl Eq for Clock {}

impl Ord for Clock {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.time_of_next() < other.time_of_next() {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }
}

impl PartialOrd for Clock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Resolver for Clock {
    fn add_resolve(&self, resolve: Rc<dyn Resolve + 'static>) {
        self.shared_state.to_resolve.borrow_mut().push(resolve);
    }
}

/// Possible states of a ClockDelay.
enum ClockDelayState {
    Pending,
    Running,
}

/// Future returned by the clock to manage advancing time using async functions.
pub struct ClockDelay {
    shared_state: Rc<ClockState>,
    until: ClockTick,
    state: ClockDelayState,
    can_exit: bool,
}

impl Future for ClockDelay {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.state {
            ClockDelayState::Pending => {
                self.shared_state.schedule(self.until, cx, self.can_exit);
                self.state = ClockDelayState::Running;
                Poll::Pending
            }
            ClockDelayState::Running => {
                self.shared_state.advance_time(self.until);
                Poll::Ready(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_to_ns() {
        let clk_ghz = Clock::new(1000.0);
        assert_eq!(1.0, clk_ghz.to_ns(&ClockTick::new().set_tick(1)));

        let slow_clk = Clock::new(0.5);
        assert_eq!(2000.0, slow_clk.to_ns(&ClockTick::new().set_tick(1)));
    }
}
