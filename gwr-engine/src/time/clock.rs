// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This module represents the time during a simulation.
//!
//! Time is made up of a cycle count and a phase.

use core::cmp::Ordering;
use std::cell::{Cell, RefCell};
use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::time::duration::AppropriateUnitDisplay;
use crate::traits::{Resolve, Resolver};

/// ClockTick structure for representing a number of Clock ticks and a phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockTick {
    /// Clock ticks.
    tick: u64,

    /// Clock phase.
    #[cfg(feature = "phase")]
    phase: u32,
}

impl ClockTick {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: 0,

            #[cfg(feature = "phase")]
            phase: 0,
        }
    }

    /// Get the current clock tick.
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Get the current clock phase.
    #[must_use]
    #[cfg(feature = "phase")]
    pub fn phase(&self) -> u32 {
        self.phase
    }

    /// Change the default constructor value of `tick`.
    pub fn set_tick(&mut self, tick: u64) -> ClockTick {
        self.tick = tick;
        *self
    }

    /// Change the default constructor value of `phase`.
    #[cfg(feature = "phase")]
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
    #[cfg(feature = "phase")]
    fn cmp(&self, other: &Self) -> Ordering {
        match self.tick.cmp(&other.tick) {
            Ordering::Greater => Ordering::Greater,
            Ordering::Less => Ordering::Less,
            Ordering::Equal => self.phase.cmp(&other.phase),
        }
    }

    #[cfg(not(feature = "phase"))]
    fn cmp(&self, other: &Self) -> Ordering {
        self.tick.cmp(&other.tick)
    }
}

impl PartialOrd for ClockTick {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Display for ClockTick {
    #[cfg(feature = "phase")]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{:?}", self.tick, self.phase)
    }
    #[cfg(not(feature = "phase"))]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.tick)
    }
}

/// State representing a clock.
#[derive(Clone)]
pub struct Clock {
    /// Frequency of the clock in MHz.
    /// *Note*: Should never be changed as it is registered at this frequency.
    freq_mhz: f64,

    pub shared_state: Rc<ClockState>,
}

impl Display for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.time_now().to_appropriate_unit_fmt(f)
    }
}

pub struct TaskWaker {
    /// Internal identifier for a scheduled clock wait.
    pub id: u64,

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

    next_waiter_id: Cell<u64>,

    /// Queue of futures waiting for the right time.
    pub waiting: RefCell<Vec<Vec<TaskWaker>>>,

    /// Queue of times at which those futures are to be woken. This is kept
    /// sorted by time so that the first entry is the next to be woken.
    pub waiting_times: RefCell<Vec<ClockTick>>,

    /// Registered [`Resolve`] functions.
    pub to_resolve: RefCell<Vec<Rc<dyn Resolve + 'static>>>,
}

impl ClockState {
    fn schedule(&self, schedule_time: ClockTick, cx: &mut Context<'_>, can_exit: bool) -> u64 {
        let waiter_id = self.next_waiter_id.get();
        self.next_waiter_id.set(waiter_id + 1);

        let mut waiting_times = self.waiting_times.borrow_mut();
        let mut waiting = self.waiting.borrow_mut();
        if let Some(index) = waiting_times.iter().position(|&x| x == schedule_time) {
            // Time already exists, add this task
            waiting[index].push(TaskWaker {
                id: waiter_id,
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
                            id: waiter_id,
                            waker: cx.waker().clone(),
                            can_exit,
                        }],
                    );
                }
                None => {
                    // Insert at the head
                    waiting_times.push(schedule_time);
                    waiting.push(vec![TaskWaker {
                        id: waiter_id,
                        waker: cx.waker().clone(),
                        can_exit,
                    }]);
                }
            }
        }

        waiter_id
    }

    fn unschedule(&self, schedule_time: ClockTick, waiter_id: u64) {
        let mut waiting_times = self.waiting_times.borrow_mut();
        let mut waiting = self.waiting.borrow_mut();

        if let Some(time_index) = waiting_times.iter().position(|&x| x == schedule_time)
            && let Some(waiter_index) = waiting[time_index].iter().position(|w| w.id == waiter_id)
        {
            waiting[time_index].remove(waiter_index);
            if waiting[time_index].is_empty() {
                waiting.remove(time_index);
                waiting_times.remove(time_index);
            }
        }
    }

    fn advance_time(&self, to_time: ClockTick) {
        self.resolve();

        assert!(to_time >= *self.now.borrow(), "Time moving backwards");
        *self.now.borrow_mut() = to_time;
    }

    fn resolve(&self) {
        for r in self.to_resolve.borrow_mut().drain(..) {
            r.resolve();
        }
    }
}

impl Clock {
    /// Create a new [Clock] at the specified frequency.
    #[must_use]
    pub fn new(freq_mhz: f64) -> Self {
        let shared_state = Rc::new(ClockState {
            now: RefCell::new(ClockTick {
                tick: 0,
                #[cfg(feature = "phase")]
                phase: 0,
            }),
            next_waiter_id: Cell::new(0),
            waiting: RefCell::new(Vec::new()),
            waiting_times: RefCell::new(Vec::new()),
            to_resolve: RefCell::new(Vec::new()),
        });

        Self {
            freq_mhz,
            shared_state,
        }
    }

    /// Advance the time on this clock
    pub fn advance_time(&self, to_time: ClockTick) {
        self.shared_state.advance_time(to_time);
    }

    /// Returns the clocks frequency in MHz.
    #[must_use]
    pub fn freq_mhz(&self) -> f64 {
        self.freq_mhz
    }

    /// Returns the current [ClockTick].
    #[must_use]
    pub fn tick_now(&self) -> ClockTick {
        *self.shared_state.now.borrow()
    }

    /// Returns the current time.
    #[must_use]
    pub fn time_now(&self) -> Duration {
        let now = *self.shared_state.now.borrow();
        self.to_duration(&now)
    }

    /// Returns the time of the next event registered with this clock.
    #[must_use]
    pub fn time_of_next(&self) -> Duration {
        match self.shared_state.waiting_times.borrow().first() {
            Some(clock_time) => self.to_duration(clock_time),
            None => Duration::MAX,
        }
    }

    fn ns_of_next(&self) -> f64 {
        match self.shared_state.waiting_times.borrow().first() {
            Some(clock_time) => self.to_ns_f64(clock_time),
            None => f64::MAX,
        }
    }

    /// Convert the given [ClockTick] to a time for this clock.
    #[must_use]
    pub fn to_duration(&self, clock_time: &ClockTick) -> Duration {
        Duration::from_secs_f64(clock_time.tick as f64 / (self.freq_mhz * 1e6))
    }

    fn to_ns_f64(&self, clock_time: &ClockTick) -> f64 {
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
            can_exit: false,
            waiter_id: None,
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
            can_exit: true,
            waiter_id: None,
        }
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    #[cfg(feature = "phase")]
    pub fn next_tick_and_phase(&self, phase: u32) -> ClockDelay {
        let mut until = self.tick_now();
        until.tick += 1;
        until.phase = phase;
        ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            can_exit: false,
            waiter_id: None,
        }
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    #[cfg(feature = "phase")]
    pub fn wait_phase(&self, phase: u32) -> ClockDelay {
        let mut until = self.tick_now();
        assert!(phase > until.phase, "Time going backwards");
        until.phase = phase;
        ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            can_exit: false,
            waiter_id: None,
        }
    }

    /// Advance to the next tick after the specified time.
    pub fn advance_to(&self, time: Duration) {
        let now = self.time_now();
        let diff_s = (time.checked_sub(now).unwrap()).as_secs_f64();
        let ticks = (diff_s * self.freq_mhz * 1e6).ceil();

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
        if self.ns_of_next() < other.ns_of_next() {
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

/// Future returned by the clock to manage advancing time using async functions.
pub struct ClockDelay {
    shared_state: Rc<ClockState>,
    until: ClockTick,
    can_exit: bool,
    waiter_id: Option<u64>,
}

impl Future for ClockDelay {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.until > *self.shared_state.now.borrow() {
            if let Some(waiter_id) = self.waiter_id {
                self.shared_state.unschedule(self.until, waiter_id);
            }
            let waiter_id = self.shared_state.schedule(self.until, cx, self.can_exit);
            self.waiter_id = Some(waiter_id);
            Poll::Pending
        } else {
            self.waiter_id = None;
            Poll::Ready(())
        }
    }
}

impl Drop for ClockDelay {
    fn drop(&mut self) {
        if let Some(waiter_id) = self.waiter_id.take() {
            self.shared_state.unschedule(self.until, waiter_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_to_ns() {
        let clk_ghz = Clock::new(1000.0);
        assert_eq!(
            Duration::from_nanos(1),
            clk_ghz.to_duration(&ClockTick::new().set_tick(1))
        );

        let slow_clk = Clock::new(0.5);
        assert_eq!(
            Duration::from_nanos(2000),
            slow_clk.to_duration(&ClockTick::new().set_tick(1))
        );
    }
}
