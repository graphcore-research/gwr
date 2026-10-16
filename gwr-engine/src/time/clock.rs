// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This module represents the time during a simulation.
//!
//! Time is made up of a tick count and a phase.

use core::cmp::Ordering;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use futures::future::FusedFuture;

use crate::traits::{Resolve, Resolver};
use crate::types::{SimError, SimResult};

pub mod phase {
    pub const BEGIN: u32 = 0;
    pub const END: u32 = u32::MAX - 2;
    /// Phase reserved for handling pending resolves.
    pub(crate) const RESOLVE: u32 = u32::MAX - 1;
    /// Between tick edges: all phases of the current tick have passed.
    pub(crate) const NONE: u32 = u32::MAX;
}

pub struct SimInstant {
    pub ns: f64,
    pub phase: u32,
}

impl Default for SimInstant {
    fn default() -> Self {
        Self {
            ns: 0.0,
            phase: phase::BEGIN,
        }
    }
}

/// ClockTick structure for representing a number of Clock ticks and a phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockTick {
    /// Clock ticks.
    tick: u64,

    /// Clock phase.
    phase: u32,
}

impl ClockTick {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: 0,
            phase: phase::BEGIN,
        }
    }

    /// Get the current clock tick.
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Get the current clock phase.
    #[must_use]
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

/// State representing a clock.
#[derive(Clone)]
pub struct Clock {
    pub shared_state: Rc<ClockState>,
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
    /// Frequency of the clock in MHz.
    /// *Note*: Should never be changed as it is registered at this frequency.
    freq_mhz: f64,

    sim_instant: Option<Rc<RefCell<SimInstant>>>,

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
    fn add_resolve(&self, resolve: Rc<dyn Resolve + 'static>) {
        self.to_resolve.borrow_mut().push(resolve);
        if self.to_resolve.borrow().len() == 1 {
            let schedule_time = self
                .deadline_for_phase(phase::RESOLVE)
                .expect("simulation clock cannot represent the required resolve deadline");

            self.ensure_scheduled_time(schedule_time);
        }
    }

    fn schedule(&self, schedule_time: ClockTick, cx: &mut Context<'_>, can_exit: bool) -> u64 {
        let waker = cx.waker().clone();
        let waiter_id = self.next_waiter_id.get();
        self.next_waiter_id.set(waiter_id + 1);

        let index = self.ensure_scheduled_time(schedule_time);
        self.waiting.borrow_mut()[index].push(TaskWaker {
            id: waiter_id,
            waker,
            can_exit,
        });

        waiter_id
    }

    fn ensure_scheduled_time(&self, schedule_time: ClockTick) -> usize {
        let mut waiting_times = self.waiting_times.borrow_mut();
        let mut waiting = self.waiting.borrow_mut();
        if let Some(index) = waiting_times.iter().position(|&time| time == schedule_time) {
            return index;
        }

        let index = waiting_times
            .iter()
            .position(|&time| time < schedule_time)
            .unwrap_or(waiting_times.len());

        waiting_times.insert(index, schedule_time);
        waiting.insert(index, Vec::new());
        index
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

    fn assert_not_past_next_event(&self, to_time: ClockTick) {
        if let Some(next_time) = self.waiting_times.borrow().last() {
            assert!(
                to_time <= *next_time,
                "Clock cannot advance past its next scheduled event: requested {to_time}, next event {next_time}"
            );
        }
    }

    fn advance_time(&self, to_time: ClockTick) {
        assert!(to_time >= *self.now.borrow(), "Time moving backwards");
        if let Some(sim_instant) = &self.sim_instant {
            let sim_instant = sim_instant.borrow();
            assert_eq!(
                (self.tick_to_ns(to_time.tick), to_time.phase),
                (sim_instant.ns, sim_instant.phase),
                "Clock advancement must match the current simulation time and phase"
            );
        }
        self.assert_not_past_next_event(to_time);
        *self.now.borrow_mut() = to_time;

        if to_time.phase() == phase::RESOLVE {
            self.resolve();
        }
    }

    fn resolve(&self) {
        for r in self.to_resolve.borrow_mut().drain(..) {
            r.resolve();
        }
    }

    fn deadline_for_phase(&self, phase: u32) -> Result<ClockTick, SimError> {
        let mut until = self.synchronize();

        if until.phase == phase::NONE {
            // When between ticks, wait for the next tick
            until.tick = until
                .tick
                .checked_add(1)
                .ok_or_else(|| SimError("clock wait phase overflows".into()))?;
        } else if phase < until.phase {
            return Err(SimError(
                "phase is before the current phase within the tick".into(),
            ));
        }

        until.phase = phase;
        Ok(until)
    }

    fn tick_to_ns(&self, tick: u64) -> f64 {
        tick as f64 / self.freq_mhz * 1000.0
    }

    fn synchronize(&self) -> ClockTick {
        let now = *self.now.borrow();
        let Some(sim_instant) = &self.sim_instant else {
            self.assert_not_past_next_event(now);
            return now;
        };

        let sim_instant = sim_instant.borrow();

        if self.tick_to_ns(now.tick) == sim_instant.ns {
            let synchronized = ClockTick {
                phase: sim_instant.phase,
                ..now
            };
            self.assert_not_past_next_event(synchronized);
            *self.now.borrow_mut() = synchronized;
            return synchronized;
        }

        let tick_estimate = sim_instant.ns * self.freq_mhz / 1000.0;
        assert!(
            tick_estimate.is_finite() && (0.0..u64::MAX as f64).contains(&tick_estimate),
            "simulation clock cannot represent the current tick"
        );

        let mut tick = tick_estimate as u64;
        while tick > 0 && self.tick_to_ns(tick) > sim_instant.ns {
            tick -= 1;
        }
        while tick < u64::MAX && self.tick_to_ns(tick + 1) <= sim_instant.ns {
            tick += 1;
        }

        // A pending event identifies an exact tick when f64 timestamps alias.
        if let Some(next_time) = self.waiting_times.borrow().last()
            && self.tick_to_ns(next_time.tick) == sim_instant.ns
        {
            tick = tick.min(next_time.tick);
        }

        assert!(
            now.tick <= tick,
            "Clock is ahead of the simulation. Clock tick: {}, Simulation tick: {}",
            now.tick,
            tick
        );

        let synchronized = ClockTick {
            tick,
            phase: if self.tick_to_ns(tick) == sim_instant.ns {
                sim_instant.phase
            } else {
                phase::NONE
            },
        };
        self.assert_not_past_next_event(synchronized);
        *self.now.borrow_mut() = synchronized;

        synchronized
    }
}

impl Clock {
    /// Create a new [Clock] at the specified frequency.
    #[must_use]
    pub fn new(freq_mhz: f64) -> Self {
        Self::new_impl(freq_mhz, None)
    }

    #[must_use]
    pub fn new_with_sim_instant(freq_mhz: f64, sim_instant: Rc<RefCell<SimInstant>>) -> Self {
        Self::new_impl(freq_mhz, Some(sim_instant))
    }

    fn new_impl(freq_mhz: f64, sim_instant: Option<Rc<RefCell<SimInstant>>>) -> Self {
        let shared_state = Rc::new(ClockState {
            freq_mhz,
            sim_instant,
            now: RefCell::new(ClockTick {
                tick: 0,
                phase: phase::BEGIN,
            }),
            next_waiter_id: Cell::new(0),
            waiting: RefCell::new(Vec::new()),
            waiting_times: RefCell::new(Vec::new()),
            to_resolve: RefCell::new(Vec::new()),
        });

        Self { shared_state }
    }

    /// Advance the time on this clock.
    pub fn advance_time(&self, to_time: ClockTick) {
        self.shared_state.advance_time(to_time);
    }

    /// Returns the clocks frequency in MHz.
    #[must_use]
    pub fn freq_mhz(&self) -> f64 {
        self.shared_state.freq_mhz
    }

    /// Returns the current [ClockTick].
    #[must_use]
    pub fn tick_now(&self) -> ClockTick {
        self.shared_state.synchronize()
    }

    /// Returns the current time in `ns`.
    #[must_use]
    pub fn time_now_ns(&self) -> f64 {
        match &self.shared_state.sim_instant {
            Some(sim_instant) => sim_instant.borrow().ns,
            None => self.to_ns(&self.shared_state.now.borrow()),
        }
    }

    /// Returns the time in `ns` of the next event registered with this clock.
    #[must_use]
    pub fn time_of_next(&self) -> f64 {
        match self.shared_state.waiting_times.borrow().last() {
            Some(clock_time) => self.to_ns(clock_time),
            None => f64::MAX,
        }
    }

    /// Returns the phase of the next event registered with this clock.
    #[must_use]
    pub fn phase_of_next(&self) -> u32 {
        match self.shared_state.waiting_times.borrow().last() {
            Some(clock_time) => clock_time.phase(),
            None => u32::MAX,
        }
    }

    /// Convert the given [ClockTick] to a time in `ns` for this clock.
    #[must_use]
    pub fn to_ns(&self, clock_time: &ClockTick) -> f64 {
        self.shared_state.tick_to_ns(clock_time.tick)
    }

    /// Returns a [ClockDelay] future which must be `await`ed to delay the
    /// specified number of ticks, rejecting a deadline that overflows.
    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn wait_ticks(&self, ticks: u64) -> Result<ClockDelay, SimError> {
        let mut until = self.tick_now();
        until.tick = until
            .tick
            .checked_add(ticks)
            .ok_or_else(|| SimError("clock wait deadline overflows".into()))?;
        until.phase = phase::BEGIN;
        Ok(ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            can_exit: false,
            waiter_id: None,
            done: false,
        })
    }

    /// Returns a [ClockDelay] future which must be `await`ed to delay the
    /// specified number of ticks. However, if the remainder of the simulation
    /// completes then this future is allowed to not complete. This allows the
    /// user to create tasks that can run continuously as long as the rest of
    /// the simulation continues to run. Returns an error if the deadline
    /// overflows.
    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn wait_ticks_or_exit(&self, ticks: u64) -> Result<ClockDelay, SimError> {
        let mut delay = self.wait_ticks(ticks)?;
        delay.can_exit = true;
        Ok(delay)
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn next_tick_and_phase(&self, phase: u32) -> Result<ClockDelay, SimError> {
        if !(phase::BEGIN..=phase::END).contains(&phase) {
            return Err(SimError("invalid phase".into()));
        }

        let mut delay = self.wait_ticks(1)?;
        delay.until.phase = phase;
        Ok(delay)
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn wait_phase(&self, phase: u32) -> Result<ClockDelay, SimError> {
        if !(phase::BEGIN..=phase::END).contains(&phase) {
            return Err(SimError("invalid phase".into()));
        }

        let until = self.shared_state.deadline_for_phase(phase)?;
        Ok(ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            can_exit: false,
            waiter_id: None,
            done: false,
        })
    }

    /// Advance to the next tick after the specified time, rejecting
    /// unrepresentable durations and tick overflow.
    pub fn advance_to(&self, time_ns: f64) -> SimResult {
        let now = self.shared_state.synchronize();
        let now_ns = self.to_ns(&now);
        assert!(now_ns < time_ns);
        let diff_ns = time_ns - now_ns;
        let ticks = (diff_ns * (self.shared_state.freq_mhz / 1000.0)).ceil();
        // u64::MAX rounds up to 2^64 as f64, so the upper bound is exclusive.
        if !ticks.is_finite() || !(0.0..u64::MAX as f64).contains(&ticks) {
            return Err(SimError(
                "clock advance duration is outside u64 range".into(),
            ));
        }

        let mut until = now;
        until.tick = until
            .tick
            .checked_add(ticks as u64)
            .ok_or_else(|| SimError("clock advance deadline overflows".into()))?;
        until.phase = phase::BEGIN;

        self.shared_state.advance_time(until);
        Ok(())
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
        self.time_of_next() == other.time_of_next() && self.phase_of_next() == other.phase_of_next()
    }
}
impl Eq for Clock {}

impl Ord for Clock {
    fn cmp(&self, other: &Self) -> Ordering {
        self.time_of_next()
            .total_cmp(&other.time_of_next())
            .then_with(|| self.phase_of_next().cmp(&other.phase_of_next()))
    }
}

impl PartialOrd for Clock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Resolver for Clock {
    fn add_resolve(&self, resolve: Rc<dyn Resolve + 'static>) {
        self.shared_state.add_resolve(resolve);
    }
}

/// Future returned by the clock to manage advancing time using async functions.
pub struct ClockDelay {
    shared_state: Rc<ClockState>,
    until: ClockTick,
    can_exit: bool,
    waiter_id: Option<u64>,
    done: bool,
}

impl Future for ClockDelay {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.done {
            return Poll::Ready(());
        }

        let now = self.shared_state.synchronize();

        if self.until > now {
            if let Some(waiter_id) = self.waiter_id {
                self.shared_state.unschedule(self.until, waiter_id);
            }
            let waiter_id = self.shared_state.schedule(self.until, cx, self.can_exit);
            self.waiter_id = Some(waiter_id);
            Poll::Pending
        } else {
            self.waiter_id = None;
            self.done = true;
            Poll::Ready(())
        }
    }
}

impl FusedFuture for ClockDelay {
    fn is_terminated(&self) -> bool {
        self.done
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
    use std::mem::drop;
    use std::pin::Pin;
    use std::task::Context;

    use futures::future::FusedFuture;
    use futures::task::noop_waker;

    use super::*;

    #[test]
    fn wait_ticks_or_exit_rejects_overflow_and_accepts_maximum_deadline() {
        let clock = Clock::default();
        clock.advance_time(ClockTick::new().set_tick(u64::MAX - 1));
        assert!(clock.wait_ticks_or_exit(2).is_err());
        let delay = clock.wait_ticks_or_exit(1).unwrap();
        assert_eq!(delay.until.tick(), u64::MAX);
        assert_eq!(delay.until.phase(), phase::BEGIN);
        assert!(delay.can_exit);
        assert!(clock.shared_state.waiting_times.borrow().is_empty());
    }

    #[test]
    fn next_tick_and_phase_rejects_overflow_and_preserves_phase() {
        let clock = Clock::default();
        clock.advance_time(ClockTick::new().set_tick(u64::MAX - 1));
        let delay = clock.next_tick_and_phase(3).unwrap();
        assert_eq!(delay.until.tick(), u64::MAX);
        assert_eq!(delay.until.phase(), 3);
        clock.advance_time(ClockTick::new().set_tick(u64::MAX));
        assert!(clock.next_tick_and_phase(3).is_err());
        assert!(clock.shared_state.waiting_times.borrow().is_empty());
    }

    #[test]
    fn advance_to_rejects_tick_overflow_without_advancing_time() {
        let clock = Clock::default();
        // The duration is a representable 2048 ticks, but its deadline is 2^64.
        let start_tick = u64::MAX - 2047;
        clock.advance_time(ClockTick::new().set_tick(start_tick));
        assert!(clock.advance_to(u64::MAX as f64).is_err());
        assert_eq!(clock.tick_now().tick(), start_tick);
    }

    #[test]
    fn advance_to_rejects_duration_at_u64_upper_bound() {
        let clock = Clock::default();
        assert!(clock.advance_to(u64::MAX as f64).is_err());
        assert_eq!(clock.tick_now(), ClockTick::default());
    }

    #[test]
    fn advance_to_rejects_infinite_duration() {
        let clock = Clock::default();
        assert!(clock.advance_to(f64::INFINITY).is_err());
        assert_eq!(clock.tick_now(), ClockTick::default());
    }

    #[test]
    fn advance_to_accepts_largest_f64_duration_below_u64_upper_bound() {
        let clock = Clock::default();
        // The largest f64 below 2^64 is 2^64 - 2048.
        let ticks = 18_446_744_073_709_549_568_u64;
        clock.advance_to(ticks as f64).unwrap();
        assert_eq!(clock.tick_now().tick(), ticks);
        assert_eq!(clock.tick_now().phase(), phase::BEGIN);
    }

    #[test]
    fn wait_ticks_rejects_deadline_overflow() {
        let clock = Clock::default();
        clock.advance_time(ClockTick::new().set_tick(u64::MAX - 1));
        assert!(clock.wait_ticks(2).is_err());
        assert!(clock.shared_state.waiting_times.borrow().is_empty());
        assert!(clock.shared_state.waiting.borrow().is_empty());
    }

    #[test]
    fn wait_ticks_accepts_maximum_deadline_and_zero_wait() {
        let clock = Clock::default();
        clock.advance_time(ClockTick::new().set_tick(u64::MAX - 1));
        let delay = clock.wait_ticks(1).unwrap();
        assert_eq!(delay.until.tick(), u64::MAX);
        clock.advance_time(ClockTick::new().set_tick(u64::MAX));
        let delay = clock.wait_ticks(0).unwrap();
        assert_eq!(delay.until.tick(), u64::MAX);
    }

    #[test]
    fn convert_to_ns() {
        let clk_ghz = Clock::new(1000.0);
        assert_eq!(1.0, clk_ghz.to_ns(&ClockTick::new().set_tick(1)));

        let slow_clk = Clock::new(0.5);
        assert_eq!(2000.0, slow_clk.to_ns(&ClockTick::new().set_tick(1)));
    }

    #[test]
    fn clock_tick_accessors_default_display_and_ordering() {
        let default_tick = ClockTick::default();
        assert_eq!(default_tick.tick(), 0);
        assert_eq!(default_tick.phase(), phase::BEGIN);
        assert_eq!(default_tick.to_string(), "0.0");

        let earlier = ClockTick::new().set_tick(1);
        let later = ClockTick::new().set_tick(2);
        assert!(earlier < later);
        assert_eq!(earlier.partial_cmp(&later), Some(Ordering::Less));
    }

    #[test]
    fn clock_tick_phase_accessors_display_and_ordering() {
        let tick = ClockTick::new().set_tick(1).set_phase(2);
        assert_eq!(tick.phase(), 2);
        assert_eq!(tick.to_string(), "1.2");

        let earlier_phase = ClockTick::new().set_tick(1).set_phase(1);
        assert!(earlier_phase < tick);

        let later_tick = ClockTick::new().set_tick(2).set_phase(0);
        assert!(later_tick > tick);
    }

    #[test]
    fn clock_default_advance_and_ordering() {
        let clock = Clock::default();
        assert_eq!(clock.freq_mhz(), 1000.0);

        clock.advance_to(2.1).unwrap();
        assert_eq!(clock.tick_now().tick(), 3);
        assert_eq!(clock.tick_now().phase(), phase::BEGIN);
        assert_eq!(clock.time_now_ns(), 3.0);

        let earlier = Clock::new(1000.0);
        let later = Clock::new(1000.0);
        drop(earlier.wait_ticks(1).unwrap());
        drop(later.wait_ticks(2).unwrap());

        assert!(earlier == later);
        assert_eq!(earlier.partial_cmp(&later), Some(Ordering::Equal));
    }

    #[test]
    fn unschedule_unknown_waiter_is_a_noop() {
        let clock = Clock::new(1000.0);
        let scheduled_time = ClockTick::new().set_tick(1);

        clock
            .shared_state
            .waiting_times
            .borrow_mut()
            .push(scheduled_time);
        clock.shared_state.waiting.borrow_mut().push(Vec::new());

        clock.shared_state.unschedule(scheduled_time, 7);

        assert_eq!(clock.shared_state.waiting_times.borrow().len(), 1);
        assert_eq!(clock.shared_state.waiting.borrow().len(), 1);
    }

    #[test]
    fn unschedule_waiter_keeps_time_when_other_waiters_remain() {
        let clock = Clock::new(1000.0);
        let scheduled_time = ClockTick::new().set_tick(1);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let first_waiter_id = clock.shared_state.schedule(scheduled_time, &mut cx, false);
        let second_waiter_id = clock.shared_state.schedule(scheduled_time, &mut cx, false);

        clock
            .shared_state
            .unschedule(scheduled_time, first_waiter_id);

        let waiting_times = clock.shared_state.waiting_times.borrow();
        let waiting = clock.shared_state.waiting.borrow();

        assert_eq!(waiting_times.as_slice(), &[scheduled_time]);
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].len(), 1);
        assert_eq!(waiting[0][0].id, second_waiter_id);
    }

    #[test]
    fn clock_delay_is_fused_after_completion() {
        let clock = Clock::new(1000.0);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut delay = clock.wait_ticks(1).unwrap();

        assert!(!delay.is_terminated());
        assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Pending);
        assert!(!delay.is_terminated());

        clock.advance_time(ClockTick::new().set_tick(1));
        assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Ready(()));
        assert!(delay.is_terminated());
        assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Ready(()));
        assert!(delay.is_terminated());
    }

    #[test]
    fn phase_wait_helpers_schedule_phase_delays() {
        let clock = Clock::new(1000.0);

        let next_tick = clock.next_tick_and_phase(3).unwrap();
        assert_eq!(next_tick.until.tick(), 1);
        assert_eq!(next_tick.until.phase(), 3);

        let same_tick = clock.wait_phase(1).unwrap();
        assert_eq!(same_tick.until.tick(), 0);
        assert_eq!(same_tick.until.phase(), 1);
    }

    #[test]
    fn wait_phase_between_ticks_targets_next_tick() {
        for (requested_phase, expected_at_begin) in
            [(phase::BEGIN, Poll::Ready(())), (phase::END, Poll::Pending)]
        {
            let clock = Clock::new(500.0);
            clock.advance_time(ClockTick::new().set_tick(4).set_phase(phase::NONE));
            let mut delay = clock.wait_phase(requested_phase).unwrap();
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);

            assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Pending);

            clock.advance_time(ClockTick::new().set_tick(5));
            assert_eq!(Pin::new(&mut delay).poll(&mut cx), expected_at_begin);

            if expected_at_begin.is_pending() {
                clock.advance_time(ClockTick::new().set_tick(5).set_phase(phase::END));
                assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Ready(()));
            }
        }
    }

    struct CountingResolve(Cell<u64>);

    impl Resolve for CountingResolve {
        fn resolve(&self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn resolve_between_ticks_is_scheduled_for_next_tick() {
        let clock = Clock::new(500.0);
        clock.advance_time(ClockTick::new().set_tick(4).set_phase(phase::NONE));
        let resolve = Rc::new(CountingResolve(Cell::new(0)));
        clock.add_resolve(resolve.clone());

        assert_eq!(clock.time_of_next(), 10.0);
        assert_eq!(clock.phase_of_next(), phase::RESOLVE);

        clock.advance_time(ClockTick::new().set_tick(5).set_phase(phase::END));
        assert_eq!(resolve.0.get(), 0);

        clock.advance_time(ClockTick::new().set_tick(5).set_phase(phase::RESOLVE));
        assert_eq!(resolve.0.get(), 1);
    }

    #[test]
    fn tick_waits_resume_in_end_phase() {
        let clock = Clock::new(1000.0);

        let begin = clock.wait_phase(phase::END).unwrap();
        assert_eq!(begin.until.tick(), 0);
        assert_eq!(begin.until.phase(), phase::END);

        clock.advance_time(ClockTick::new().set_phase(phase::END));

        let delay = clock.wait_ticks(1).unwrap();
        assert_eq!(delay.until.tick(), 1);
        assert_eq!(delay.until.phase(), phase::BEGIN);
    }
}
