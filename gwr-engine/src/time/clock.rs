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
use crate::types::SimError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Frequency {
    significand: u64,
    exponent: i16,
}

impl Frequency {
    pub(super) fn new(freq_mhz: f64) -> Self {
        assert!(freq_mhz.is_finite());
        assert!(freq_mhz > 0.0);

        let bits = freq_mhz.to_bits();

        let mut significand = bits & ((1u64 << 52) - 1);
        let mut exponent = ((bits >> 52) & ((1 << 11) - 1)) as i16;

        if exponent == 0 {
            // Subnormal numbers
            exponent = -1074;
        } else {
            // Normal numbers
            significand |= 1u64 << 52;
            exponent -= 1075;
        }

        let trailing_zeros = significand.trailing_zeros();
        significand >>= trailing_zeros;
        exponent += trailing_zeros as i16;

        Self {
            significand,
            exponent,
        }
    }

    /// Compare the time of a tick at this frequency against a given tick at
    /// another frequency.
    pub(super) fn cmp_ticks(&self, tick: u64, other: &Self, other_tick: u64) -> Ordering {
        if self == other {
            return tick.cmp(&other_tick);
        }

        // We want to know if:
        //   tick / self < other_tick / other
        // Which is equivalent to:
        //   tick * other < other_tick * self
        let a = u128::from(tick) * u128::from(other.significand);
        let a_shift = other.exponent;
        let b = u128::from(other_tick) * u128::from(self.significand);
        let b_shift = self.exponent;

        cmp_scaled(a, a_shift, b, b_shift)
    }

    /// Get the last tick at this frequency not later than a given tick at
    /// another frequency, and whether the ticks coincide.
    ///
    /// Returns None if the conversion would overflow.
    pub(super) fn tick_at(&self, other: &Self, other_tick: u64) -> Option<(u64, bool)> {
        // We want to calculate floor(other_tick / other * self)
        if other_tick == 0 || self == other {
            return Some((other_tick, true));
        }

        let numerator = u128::from(other_tick) * u128::from(self.significand);
        let denominator = u128::from(other.significand);
        let exponent = self.exponent - other.exponent;

        // Return nothing if (numerator << exponent) / denominator >= 2^64
        if cmp_scaled(numerator, exponent, denominator, 64).is_ge() {
            return None;
        }

        let (tick, remainder) = if exponent >= 0 {
            let shift = exponent as u32;
            let numerator = numerator << shift;
            (numerator / denominator, numerator % denominator)
        } else {
            let shift = exponent.unsigned_abs() as u32;

            // If the denominator needs more bits than the numerator, then the
            // fraction will lie in (0, 1) - i.e. the result is tick 0.
            let denominator_bits = u128::BITS - denominator.leading_zeros() + shift;
            let numerator_bits = u128::BITS - numerator.leading_zeros();
            if denominator_bits > numerator_bits {
                return Some((0, false));
            }

            let denominator = denominator << shift;
            (numerator / denominator, numerator % denominator)
        };

        Some((tick as u64, remainder == 0))
    }
}

/// Compare `a * 2^a_shift` with `b * 2^b_shift` exactly.
fn cmp_scaled(a: u128, a_shift: i16, b: u128, b_shift: i16) -> Ordering {
    if a_shift == b_shift || a == 0 || b == 0 {
        return a.cmp(&b);
    }

    // Compare the bit lengths adjusted by their exponents. If these differ,
    // the scaled values have different highest set-bit positions, which
    // determines their order without needing to shift either operand.
    let bits_a = (u128::BITS - a.leading_zeros()).cast_signed() + a_shift as i32;
    let bits_b = (u128::BITS - b.leading_zeros()).cast_signed() + b_shift as i32;
    let magnitude = bits_a.cmp(&bits_b);
    if magnitude != Ordering::Equal {
        return magnitude;
    }

    // Factor out the smaller power of two, leaving only a relative left shift.
    // This avoids losing bits through right shifts when exponents are negative.
    // Equal adjusted bit lengths guarantee that the shifted value has the same
    // bit length as the other operand, so it fits in u128 and the shift is <
    // 128.
    let relative_shift = a_shift.abs_diff(b_shift);
    if a_shift > b_shift {
        (a << relative_shift).cmp(&b)
    } else {
        a.cmp(&(b << relative_shift))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SimInstant {
    pub freq: Frequency,
    pub tick: u64,
    pub phase: u32,

    /// Precomputed ns timestamp, for reporting only.
    pub ns: f64,
}

impl SimInstant {
    pub(super) fn new() -> Self {
        Self {
            freq: Frequency::new(1000.0),
            tick: 0,
            phase: phase::BEGIN,
            ns: 0.0,
        }
    }

    /// Compare the time of this instant against the time of another instant,
    /// ignoring phase.
    pub(super) fn cmp_tick(self, other: &SimInstant) -> Ordering {
        self.freq.cmp_ticks(self.tick, &other.freq, other.tick)
    }
}

pub mod phase {
    pub const BEGIN: u32 = 0;
    pub const END: u32 = u32::MAX - 2;
    /// A reserved phase for handling [`Resolve`](crate::traits::Resolve)s.
    pub(super) const RESOLVE: u32 = u32::MAX - 1;
    /// The clock is between ticks.
    ///
    /// That is, simulation time is strictly between tick times of the clock.
    ///
    /// For example, given a 500MHz clock which ticks at 0, 2, 4, ... ns,
    /// when simulation time is 3ns, then the clock will be considered between
    /// ticks and its phase will be `NONE`.
    pub const NONE: u32 = u32::MAX;
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
    shared_state: Rc<ClockState>,
}

pub(super) struct TaskWaker {
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
struct ClockState {
    /// Frequency of the clock in MHz.
    /// *Note*: Should never be changed as it is registered at this frequency.
    freq_mhz: f64,

    freq: Frequency,

    sim_instant: Rc<Cell<SimInstant>>,

    last_sync: Cell<SimInstant>,

    now: Cell<ClockTick>,

    next_waiter_id: Cell<u64>,

    /// Queue of futures waiting for the right time.
    waiting: RefCell<Vec<Vec<TaskWaker>>>,

    /// Queue of times at which those futures are to be woken. This is kept
    /// sorted by time so that the last entry is the next to be woken.
    waiting_times: RefCell<Vec<ClockTick>>,

    /// Registered [`Resolve`] functions.
    to_resolve: RefCell<Vec<Rc<dyn Resolve + 'static>>>,
}

impl ClockState {
    #[allow(
        clippy::inline_always,
        reason = "Inlining the cached check measurably reduces clock wait overhead"
    )]
    #[inline(always)]
    fn synchronize(&self) -> ClockTick {
        let sim_instant = self.sim_instant.get();

        {
            let last_sync = self.last_sync.get();
            if sim_instant == last_sync {
                return self.now.get();
            }
        }

        self.do_synchronize(&sim_instant)
    }

    #[inline(never)]
    fn do_synchronize(&self, sim_instant: &SimInstant) -> ClockTick {
        let (tick, in_phase) = self
            .freq
            .tick_at(&sim_instant.freq, sim_instant.tick)
            .expect("sim instant to tick overflows");

        let phase = if in_phase {
            sim_instant.phase
        } else {
            phase::NONE
        };

        let synchronized = ClockTick { tick, phase };

        // Confirm the clock hasn't advanced past a scheduled event.
        if let Some(next_event_time) = self.waiting_times.borrow().last() {
            assert!(synchronized <= *next_event_time);
        }

        self.now.set(synchronized);
        self.last_sync.set(*sim_instant);

        synchronized
    }

    fn target_time_for_phase(&self, phase: u32) -> Result<ClockTick, SimError> {
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

    fn schedule(&self, schedule_time: ClockTick, cx: &mut Context<'_>, can_exit: bool) -> u64 {
        let waker = cx.waker().clone();
        let waiter_id = self.next_waiter_id.get();
        self.next_waiter_id.set(waiter_id + 1);

        let task_waker = TaskWaker {
            id: waiter_id,
            waker,
            can_exit,
        };
        let mut waiting_times = self.waiting_times.borrow_mut();
        let mut waiting = self.waiting.borrow_mut();

        match waiting_times.binary_search_by(|time| schedule_time.cmp(time)) {
            Ok(index) => waiting[index].push(task_waker),
            Err(index) => {
                waiting_times.insert(index, schedule_time);
                waiting.insert(index, vec![task_waker]);
            }
        }

        waiter_id
    }

    #[allow(
        clippy::inline_always,
        reason = "Inlining the absent-registration check avoids the removal path's stack setup"
    )]
    #[inline(always)]
    fn unschedule(&self, schedule_time: ClockTick, waiter_id: u64) {
        let mut waiting_times = self.waiting_times.borrow_mut();
        if waiting_times
            .last()
            .is_none_or(|time| *time > schedule_time)
        {
            return;
        }

        self.do_unschedule(&mut waiting_times, schedule_time, waiter_id);
    }

    #[inline(never)]
    fn do_unschedule(
        &self,
        waiting_times: &mut Vec<ClockTick>,
        schedule_time: ClockTick,
        waiter_id: u64,
    ) {
        let time_index = match waiting_times.binary_search_by(|time| schedule_time.cmp(time)) {
            Ok(index) => index,
            Err(_) => {
                return;
            }
        };

        let mut waiting = self.waiting.borrow_mut();
        if let Some(waiter_index) = waiting[time_index].iter().position(|w| w.id == waiter_id) {
            waiting[time_index].remove(waiter_index);
            if waiting[time_index].is_empty() {
                waiting.remove(time_index);
                waiting_times.remove(time_index);
            }
        }
    }

    fn add_resolve(&self, resolve: Rc<dyn Resolve + 'static>) {
        {
            let mut to_resolve = self.to_resolve.borrow_mut();
            to_resolve.push(resolve);
            if to_resolve.len() > 1 {
                return;
            }
        }

        let schedule_time = self
            .target_time_for_phase(phase::RESOLVE)
            .expect("simulation clock cannot represent the required resolve deadline");

        let mut waiting_times = self.waiting_times.borrow_mut();
        let mut waiting = self.waiting.borrow_mut();

        match waiting_times.binary_search_by(|time| schedule_time.cmp(time)) {
            Ok(_) => unreachable!("multiple resolve events scheduled"),
            Err(index) => {
                waiting_times.insert(index, schedule_time);
                waiting.insert(index, vec![]);
            }
        }
    }

    fn tick_to_ns(&self, tick: u64) -> f64 {
        tick as f64 / self.freq_mhz * 1000.0
    }
}

impl Clock {
    /// Create a new [Clock] at the specified frequency.
    #[must_use]
    pub(super) fn new(freq_mhz: f64, sim_instant: Rc<Cell<SimInstant>>) -> Self {
        let shared_state = Rc::new(ClockState {
            freq_mhz,
            freq: Frequency::new(freq_mhz),
            sim_instant,
            last_sync: Cell::new(SimInstant::new()),
            now: Cell::new(ClockTick {
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

    /// Whether this clock has scheduled events outstanding.
    pub(super) fn has_scheduled_events(&self) -> bool {
        !self.shared_state.waiting_times.borrow().is_empty()
    }

    /// Whether this clock has any required events outstanding, e.g.
    /// non-background tasks or resolves.
    pub(super) fn can_exit(&self) -> bool {
        if !self.shared_state.to_resolve.borrow().is_empty() {
            return false;
        }

        for task_waker in self.shared_state.waiting.borrow().iter().flatten() {
            if !task_waker.can_exit {
                return false;
            }
        }

        true
    }

    /// Advance this clock to its next pending event.
    pub(super) fn advance_to_event(&self) {
        assert!(self.has_scheduled_events());

        // Pop the next scheduled event
        let event_time = self.shared_state.waiting_times.borrow_mut().pop().unwrap();
        let event_wakers = self.shared_state.waiting.borrow_mut().pop().unwrap();

        // The sim time should have been advanced already
        let sim_instant = self.shared_state.sim_instant.get();
        assert_eq!(sim_instant.freq, self.shared_state.freq);
        assert_eq!(sim_instant.tick, event_time.tick);
        assert_eq!(sim_instant.phase, event_time.phase);

        // Sanity check the event
        match event_time.phase {
            phase::BEGIN..=phase::END => assert!(!event_wakers.is_empty()),
            phase::RESOLVE => {
                assert!(event_wakers.is_empty());
                assert!(!self.shared_state.to_resolve.borrow().is_empty());
            }
            _ => unreachable!("unexpected event phase: {}", event_time.phase),
        }

        // Advance to the event
        self.shared_state.now.replace(event_time);
        self.shared_state.last_sync.replace(sim_instant);

        // Handle the event
        match event_time.phase {
            phase::BEGIN..=phase::END => {
                for task_waker in event_wakers {
                    task_waker.waker.wake();
                }
            }
            phase::RESOLVE => {
                for r in self.shared_state.to_resolve.borrow_mut().drain(..) {
                    r.resolve();
                }
            }
            _ => (), // Do nothing
        }
    }

    /// Compare next scheduled event time against a given clock.
    pub(super) fn cmp_next_event(&self, other: &Clock) -> Ordering {
        match (
            self.shared_state.waiting_times.borrow().last(),
            other.shared_state.waiting_times.borrow().last(),
        ) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(tick), Some(other_tick)) => self
                .shared_state
                .freq
                .cmp_ticks(tick.tick, &other.shared_state.freq, other_tick.tick)
                .then_with(|| tick.phase.cmp(&other_tick.phase)),
        }
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
        self.shared_state.sim_instant.get().ns
    }

    /// Returns the sim instant of the next event registered with this clock.
    #[must_use]
    pub(super) fn instant_of_next(&self) -> Option<SimInstant> {
        self.shared_state
            .waiting_times
            .borrow()
            .last()
            .map(|clock_time| SimInstant {
                freq: self.shared_state.freq,
                tick: clock_time.tick,
                phase: clock_time.phase,
                ns: self.to_ns(clock_time),
            })
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
        debug_assert!((phase::BEGIN..=phase::END).contains(&phase));
        let mut delay = self.wait_ticks(1)?;
        delay.until.phase = phase;
        Ok(delay)
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn wait_phase(&self, phase: u32) -> Result<ClockDelay, SimError> {
        debug_assert!((phase::BEGIN..=phase::END).contains(&phase));
        let until = self.shared_state.target_time_for_phase(phase)?;
        Ok(ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            can_exit: false,
            waiter_id: None,
            done: false,
        })
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

        if self.until > self.shared_state.synchronize() {
            if let Some(waiter_id) = self.waiter_id {
                self.shared_state.unschedule(self.until, waiter_id);
            }
            let waiter_id = self.shared_state.schedule(self.until, cx, self.can_exit);
            self.waiter_id = Some(waiter_id);
            Poll::Pending
        } else {
            if let Some(waiter_id) = self.waiter_id.take() {
                self.shared_state.unschedule(self.until, waiter_id);
            }
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

    fn new_clock(freq_mhz: f64) -> Clock {
        Clock::new(freq_mhz, Rc::new(Cell::new(SimInstant::new())))
    }

    fn clock_at_tick(tick: u64) -> Clock {
        let clock = new_clock(1000.0);
        let now = ClockTick::new().set_tick(tick);
        let instant = SimInstant {
            freq: clock.shared_state.freq,
            tick,
            ns: clock.to_ns(&now),
            phase: now.phase(),
        };
        // Seed an exact tick for deadline arithmetic without reconstructing it
        // from a floating-point timestamp near u64::MAX.
        clock.shared_state.sim_instant.replace(instant);
        clock.shared_state.now.replace(now);
        clock.shared_state.last_sync.replace(instant);
        clock
    }

    fn advance_to_next_event(clock: &Clock) {
        clock
            .shared_state
            .sim_instant
            .replace(clock.instant_of_next().unwrap());
        clock.advance_to_event();
    }

    #[test]
    fn wait_ticks_or_exit_rejects_overflow_and_accepts_maximum_deadline() {
        let clock = clock_at_tick(u64::MAX - 1);
        assert!(clock.wait_ticks_or_exit(2).is_err());
        let delay = clock.wait_ticks_or_exit(1).unwrap();
        assert_eq!(delay.until.tick(), u64::MAX);
        assert_eq!(delay.until.phase(), phase::BEGIN);
        assert!(delay.can_exit);
        assert!(clock.shared_state.waiting_times.borrow().is_empty());
    }

    #[test]
    fn next_tick_and_phase_rejects_overflow_and_preserves_phase() {
        let clock = clock_at_tick(u64::MAX - 1);
        let delay = clock.next_tick_and_phase(3).unwrap();
        assert_eq!(delay.until.tick(), u64::MAX);
        assert_eq!(delay.until.phase(), 3);
        let clock = clock_at_tick(u64::MAX);
        assert!(clock.next_tick_and_phase(3).is_err());
        assert!(clock.shared_state.waiting_times.borrow().is_empty());
    }

    #[test]
    fn wait_ticks_rejects_deadline_overflow() {
        let clock = clock_at_tick(u64::MAX - 1);
        assert!(clock.wait_ticks(2).is_err());
        assert!(clock.shared_state.waiting_times.borrow().is_empty());
        assert!(clock.shared_state.waiting.borrow().is_empty());
    }

    #[test]
    fn wait_ticks_accepts_maximum_deadline_and_zero_wait() {
        let clock = clock_at_tick(u64::MAX - 1);
        let delay = clock.wait_ticks(1).unwrap();
        assert_eq!(delay.until.tick(), u64::MAX);
        let clock = clock_at_tick(u64::MAX);
        let delay = clock.wait_ticks(0).unwrap();
        assert_eq!(delay.until.tick(), u64::MAX);
    }

    #[test]
    fn convert_to_ns() {
        let clk_ghz = new_clock(1000.0);
        assert_eq!(1.0, clk_ghz.to_ns(&ClockTick::new().set_tick(1)));

        let slow_clk = new_clock(0.5);
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
    fn clock_starts_at_zero_and_compares_next_events() {
        let clock = new_clock(1000.0);
        assert_eq!(clock.freq_mhz(), 1000.0);
        assert_eq!(clock.tick_now().tick(), 0);
        assert_eq!(clock.tick_now().phase(), phase::BEGIN);
        assert_eq!(clock.time_now_ns(), 0.0);

        let earlier = new_clock(1000.0);
        let later = new_clock(1000.0);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut earlier_wait = earlier.wait_ticks(1).unwrap();
        let mut later_wait = later.wait_ticks(2).unwrap();
        assert_eq!(Pin::new(&mut earlier_wait).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut later_wait).poll(&mut cx), Poll::Pending);
        assert_eq!(earlier.cmp_next_event(&later), Ordering::Less);

        drop(earlier_wait);
        drop(later_wait);
        assert_eq!(earlier.cmp_next_event(&later), Ordering::Equal);
    }

    #[test]
    fn unschedule_unknown_waiter_is_a_noop() {
        let clock = new_clock(1000.0);
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
        let clock = new_clock(1000.0);
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
        let clock = new_clock(1000.0);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut delay = clock.wait_ticks(1).unwrap();

        assert!(!delay.is_terminated());
        assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Pending);
        assert!(!delay.is_terminated());

        advance_to_next_event(&clock);
        assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Ready(()));
        assert!(delay.is_terminated());
        assert_eq!(Pin::new(&mut delay).poll(&mut cx), Poll::Ready(()));
        assert!(delay.is_terminated());
    }

    #[test]
    fn phase_wait_helpers_schedule_phase_delays() {
        let clock = new_clock(1000.0);

        let next_tick = clock.next_tick_and_phase(3).unwrap();
        assert_eq!(next_tick.until.tick(), 1);
        assert_eq!(next_tick.until.phase(), 3);

        let same_tick = clock.wait_phase(1).unwrap();
        assert_eq!(same_tick.until.tick(), 0);
        assert_eq!(same_tick.until.phase(), 1);
    }

    #[test]
    fn tick_waits_resume_in_begin_phase_after_end_phase() {
        let clock = new_clock(1000.0);

        let mut begin = clock.wait_phase(phase::END).unwrap();
        assert_eq!(begin.until.tick(), 0);
        assert_eq!(begin.until.phase(), phase::END);

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert_eq!(Pin::new(&mut begin).poll(&mut cx), Poll::Pending);
        advance_to_next_event(&clock);
        assert_eq!(Pin::new(&mut begin).poll(&mut cx), Poll::Ready(()));

        let delay = clock.wait_ticks(1).unwrap();
        assert_eq!(delay.until.tick(), 1);
        assert_eq!(delay.until.phase(), phase::BEGIN);
    }
}
