// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This module represents the time during a simulation.
//!
//! Time is made up of a tick count and a phase.

use core::cmp::Ordering;
use std::cell::{Cell, RefCell};
use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use futures::future::FusedFuture;
use gwr_std::error::{SimError, SimResult};
use gwr_std::time::Duration;

use crate::traits::{Resolve, Resolver};

const HERTZ_PER_MHZ: f64 = 1_000_000.0;
const NANOS_PER_SEC: f64 = 1_000_000_000.0;

pub mod phase {
    pub const BEGIN: u32 = 0;
    pub const END: u32 = u32::MAX;
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

impl Display for ClockTick {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{:?}", self.tick, self.phase)
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
    /// sorted by time so that the last entry is the next to be woken.
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
        let task_waker = TaskWaker {
            id: waiter_id,
            waker: cx.waker().clone(),
            can_exit,
        };
        // Deadlines are descending so the earliest bucket can be popped from
        // the back.
        match waiting_times.binary_search_by(|time| schedule_time.cmp(time)) {
            Ok(index) => waiting[index].push(task_waker),
            Err(index) => {
                waiting_times.insert(index, schedule_time);
                waiting.insert(index, vec![task_waker]);
            }
        }

        waiter_id
    }

    fn unschedule(&self, schedule_time: ClockTick, waiter_id: u64) {
        let mut waiting_times = self.waiting_times.borrow_mut();
        let mut waiting = self.waiting.borrow_mut();

        if let Ok(time_index) = waiting_times.binary_search_by(|time| schedule_time.cmp(time))
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
                phase: phase::BEGIN,
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
        match self.shared_state.waiting_times.borrow().last() {
            Some(clock_time) => self.to_duration(clock_time),
            None => Duration::MAX,
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

    /// Convert the given [ClockTick] to a time for this clock.
    #[must_use]
    pub fn to_duration(&self, clock_time: &ClockTick) -> Duration {
        self.try_to_duration(clock_time.tick)
            .expect("clock time is outside the simulation duration range")
    }

    fn try_to_duration(&self, tick: u64) -> Result<Duration, SimError> {
        if let Some(hertz) = self.integral_hz() {
            let seconds = u32::try_from(tick / hertz)
                .map_err(|_| SimError("clock time exceeds the simulation duration range".into()))?;
            let nanoseconds = (tick % hertz) as f64 * (NANOS_PER_SEC / hertz as f64);
            Duration::try_new(seconds, nanoseconds)
        } else {
            Duration::try_from_secs_f64(tick as f64 / (self.freq_mhz * HERTZ_PER_MHZ))
        }
    }

    fn integral_hz(&self) -> Option<u64> {
        let hertz = self.freq_mhz * HERTZ_PER_MHZ;
        if hertz > 0.0 && hertz < u64::MAX as f64 && hertz.fract() == 0.0 {
            Some(hertz as u64)
        } else {
            None
        }
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
        let mut delay = self.wait_ticks(1)?;
        delay.until.phase = phase;
        Ok(delay)
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn wait_phase(&self, phase: u32) -> ClockDelay {
        let mut until = self.tick_now();
        assert!(phase >= until.phase, "Time going backwards");
        until.phase = phase;
        ClockDelay {
            shared_state: self.shared_state.clone(),
            until,
            can_exit: false,
            waiter_id: None,
            done: false,
        }
    }

    /// Advance to the next tick after the specified time, rejecting
    /// unrepresentable durations and tick overflow.
    pub fn advance_to(&self, time: Duration) -> SimResult {
        let now = self.time_now();
        assert!(now < time);
        let mut until = self.tick_now();
        let mut first_tick = until
            .tick
            .checked_add(1)
            .ok_or_else(|| SimError("clock advance deadline overflows".into()))?;
        let mut last_tick = u64::MAX;
        let mut next_tick = None;

        while first_tick <= last_tick {
            let midpoint = first_tick.midpoint(last_tick);
            match self.try_to_duration(midpoint) {
                Ok(duration) if duration < time => {
                    let Some(after) = midpoint.checked_add(1) else {
                        break;
                    };
                    first_tick = after;
                }
                result => {
                    if result.is_ok() {
                        next_tick = Some(midpoint);
                    }
                    last_tick = midpoint - 1;
                }
            }
        }

        until.tick = next_tick.ok_or_else(|| {
            SimError("clock advance target has no representable future tick".into())
        })?;
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
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Clock {}

impl Ord for Clock {
    fn cmp(&self, other: &Self) -> Ordering {
        let next = self.shared_state.waiting_times.borrow().last().copied();
        let other_next = other.shared_state.waiting_times.borrow().last().copied();
        match (next, other_next) {
            (Some(next), Some(other_next)) => self
                .to_duration(&next)
                .partial_cmp(&other.to_duration(&other_next))
                .expect("validated clock durations are comparable")
                .then_with(|| next.phase.cmp(&other_next.phase)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
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
    done: bool,
}

impl Future for ClockDelay {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.done {
            return Poll::Ready(());
        }

        if self.until > *self.shared_state.now.borrow() {
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

    fn assert_advance_reaches_first_tick(clock: &Clock, target: Duration) {
        let start = clock.tick_now().tick();
        clock.advance_to(target).unwrap();
        let advanced = clock.tick_now();
        assert!(advanced.tick() > start);
        assert_eq!(advanced.phase(), phase::BEGIN);
        assert!(clock.time_now() >= target);
        let predecessor = advanced.tick() - 1;
        if predecessor > start {
            assert!(clock.to_duration(&ClockTick::new().set_tick(predecessor)) < target);
        }
    }

    #[test]
    fn advance_to_roundtrips_fractional_tick_deadline() {
        let clock = Clock::new(1800.0);
        let target = clock.to_duration(&ClockTick::new().set_tick(7));
        assert_advance_reaches_first_tick(&clock, target);
        assert_eq!(clock.tick_now().tick(), 7);
    }

    #[test]
    fn advance_to_chooses_minimal_ticks_around_represented_deadlines() {
        for freq_mhz in [1000.0, 1800.0, 2000.0, 8000.0] {
            for start in [0, 3, 1_u64 << 53] {
                let clock = Clock::new(freq_mhz);
                let deadline = clock.to_duration(&ClockTick::new().set_tick(start + 7));
                let nanos = deadline.subsec_nanos();
                for target_nanos in [nanos.next_down(), nanos, nanos.next_up()] {
                    let clock = Clock::new(freq_mhz);
                    clock.advance_time(ClockTick::new().set_tick(start).set_phase(phase::END));
                    let target = Duration::new(deadline.as_secs(), target_nanos);
                    assert_advance_reaches_first_tick(&clock, target);
                }
            }
        }
    }

    #[test]
    fn advance_to_chooses_first_tick_on_fractional_hertz_plateau() {
        let clock = Clock::new(1_000.000_000_5);
        let ticks = 1_u64 << 53;
        let target = clock.to_duration(&ClockTick::new().set_tick(ticks + 4));
        assert_eq!(
            target,
            clock.to_duration(&ClockTick::new().set_tick(ticks + 3)),
        );
        clock.advance_time(ClockTick::new().set_tick(ticks - 4));
        assert_advance_reaches_first_tick(&clock, target);
        assert_eq!(clock.tick_now().tick(), ticks + 3);
    }

    #[test]
    fn advance_to_rejects_exhausted_clock_without_advancing() {
        let clock = Clock::new(8000.0);
        let start = ClockTick::new().set_tick(u64::MAX).set_phase(phase::END);
        clock.advance_time(start);
        let target = clock.time_now() + Duration::new(0, 0.125);
        assert!(clock.advance_to(target).is_err());
        assert_eq!(clock.tick_now(), start);
    }

    #[test]
    fn advance_to_searches_past_out_of_range_ticks() {
        let clock = Clock::new(0.000_001);
        let target = Duration::from_secs(u32::MAX);
        assert_advance_reaches_first_tick(&clock, target);
        assert_eq!(clock.tick_now().tick(), u64::from(u32::MAX));
    }

    #[test]
    fn advance_to_rejects_unrepresentable_final_tick_without_advancing() {
        let clock = Clock::new(0.000_001);
        let start = ClockTick::new().set_phase(phase::END);
        clock.advance_time(start);
        assert!(clock.advance_to(Duration::MAX).is_err());
        assert_eq!(clock.tick_now(), start);
    }

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
        let clock = Clock::new(8000.0);
        let start_tick = u64::MAX - 2047;
        clock.advance_time(ClockTick::new().set_tick(start_tick));
        let target = Duration::new(2_305_843_009, 213_693_952.0);
        assert!(clock.advance_to(target).is_err());
        assert_eq!(clock.tick_now().tick(), start_tick);
    }

    #[test]
    fn advance_to_rejects_duration_at_u64_upper_bound() {
        let clock = Clock::new(8000.0);
        let target = Duration::new(2_305_843_009, 213_693_952.0);
        assert!(clock.advance_to(target).is_err());
        assert_eq!(clock.tick_now(), ClockTick::default());
    }

    #[test]
    fn advance_to_rejects_duration_outside_tick_range() {
        let clock = Clock::new(8000.0);
        assert!(clock.advance_to(Duration::MAX).is_err());
        assert_eq!(clock.tick_now(), ClockTick::default());
    }

    #[test]
    fn advance_to_accepts_maximum_tick_deadline() {
        let clock = Clock::new(8000.0);
        let target = clock.to_duration(&ClockTick::new().set_tick(u64::MAX));
        clock.advance_to(target).unwrap();
        assert_eq!(clock.tick_now().tick(), u64::MAX);
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
    fn advance_to_accepts_exact_tick_deadlines() {
        for ticks in [1, 2, 13] {
            let clock = Clock::default();
            clock.advance_to(Duration::from_nanos(ticks)).unwrap();
            assert_eq!(clock.tick_now().tick(), ticks);
        }
    }

    #[test]
    fn advance_to_rounds_up_to_next_tick() {
        let clock = Clock::new(100.0);
        clock.advance_to(Duration::from_nanos(21)).unwrap();
        assert_eq!(clock.tick_now().tick(), 3);
        assert_eq!(clock.time_now(), Duration::from_nanos(30));
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
    fn convert_to_duration() {
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

    #[test]
    fn convert_to_duration_preserves_exact_second_boundary() {
        let clock = Clock::default();
        assert_eq!(
            clock.to_duration(&ClockTick::new().set_tick(1_000_000_001)),
            Duration::new(1, 1.0),
        );
    }

    #[test]
    fn convert_to_duration_preserves_subnanosecond_ticks() {
        for (freq_mhz, nanoseconds) in [(2000.0, 0.5), (4000.0, 0.25)] {
            let clock = Clock::new(freq_mhz);
            assert_eq!(
                clock.to_duration(&ClockTick::new().set_tick(1)),
                Duration::new(0, nanoseconds),
            );
        }
    }

    #[test]
    fn convert_to_duration_preserves_adjacent_long_ticks() {
        let ticks = 1_u64 << 53;
        for (freq_mhz, nanoseconds_per_tick) in [(1000.0, 1.0), (2000.0, 0.5)] {
            let clock = Clock::new(freq_mhz);
            let earlier = clock.to_duration(&ClockTick::new().set_tick(ticks));
            let later = clock.to_duration(&ClockTick::new().set_tick(ticks + 1));
            assert!(earlier < later);
            assert_eq!(
                later.checked_sub(earlier).unwrap(),
                Duration::new(0, nanoseconds_per_tick),
            );
        }
    }

    #[test]
    fn advance_to_preserves_long_time_fraction() {
        let ticks = 1_u64 << 53;
        for (freq_mhz, nanoseconds_per_tick) in [(1000.0, 1.0), (2000.0, 0.5)] {
            let clock = Clock::new(freq_mhz);
            clock.advance_time(ClockTick::new().set_tick(ticks));
            let target = clock.time_now() + Duration::new(0, nanoseconds_per_tick);
            clock.advance_to(target).unwrap();
            assert_eq!(clock.tick_now().tick(), ticks + 1);
            assert_eq!(clock.time_now(), target);
        }
    }

    #[test]
    fn advance_to_preserves_whole_seconds_and_fraction() {
        let clock = Clock::new(2000.0);
        let target = Duration::new(10_000_000, 0.5);
        clock.advance_to(target).unwrap();
        assert_eq!(clock.tick_now().tick(), 20_000_000_000_000_001);
        assert_eq!(clock.time_now(), target);
    }

    #[test]
    fn advance_to_rounds_subnanosecond_deadline_up() {
        let clock = Clock::new(4000.0);
        clock.advance_to(Duration::new(0, 0.1)).unwrap();
        assert_eq!(clock.tick_now().tick(), 1);
        assert_eq!(clock.time_now(), Duration::new(0, 0.25));
    }

    #[test]
    fn fractional_hertz_clock_preserves_approximate_conversion() {
        let clock = Clock::new(0.000_001_5);
        let target = Duration::from_secs_f64(0.5);
        clock.advance_to(target).unwrap();
        assert_eq!(clock.tick_now().tick(), 1);
        assert_eq!(clock.time_now(), Duration::from_secs_f64(1.0 / 1.5));
    }

    #[test]
    fn clock_ordering_preserves_long_mixed_clock_deadlines_before_phase() {
        let ticks = 1_u64 << 53;
        let earlier = Clock::new(2000.0);
        let later = Clock::default();
        earlier.shared_state.waiting_times.borrow_mut().push(
            ClockTick::new()
                .set_tick(ticks * 2 + 1)
                .set_phase(phase::END),
        );
        later
            .shared_state
            .waiting_times
            .borrow_mut()
            .push(ClockTick::new().set_tick(ticks + 1).set_phase(phase::BEGIN));
        assert!(earlier < later);
    }

    #[test]
    fn clock_ordering_uses_phase_at_equal_long_deadlines() {
        let ticks = 1_u64 << 53;
        let earlier = Clock::new(2000.0);
        let later = Clock::default();
        earlier
            .shared_state
            .waiting_times
            .borrow_mut()
            .push(ClockTick::new().set_tick(ticks * 2).set_phase(1));
        later
            .shared_state
            .waiting_times
            .borrow_mut()
            .push(ClockTick::new().set_tick(ticks).set_phase(2));
        assert_eq!(earlier.time_of_next(), later.time_of_next());
        assert!(earlier < later);
    }

    #[test]
    fn clock_without_waiters_orders_after_latest_deadline_and_phase() {
        let scheduled = Clock::new(0.000_001);
        let empty = Clock::default();
        scheduled.shared_state.waiting_times.borrow_mut().push(
            ClockTick::new()
                .set_tick(u64::from(u32::MAX))
                .set_phase(phase::END),
        );
        assert_eq!(empty.time_of_next(), Duration::MAX);
        assert_eq!(empty.phase_of_next(), phase::END);
        assert!(scheduled < empty);
        assert!(empty > scheduled);
        assert!(empty == Clock::new(2000.0));
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

        clock.advance_to(Duration::from_nanos(3)).unwrap();
        assert_eq!(clock.tick_now().tick(), 3);
        assert_eq!(clock.tick_now().phase(), phase::BEGIN);
        assert_eq!(clock.time_now(), Duration::from_nanos(3));

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

        let same_tick = clock.wait_phase(1);
        assert_eq!(same_tick.until.tick(), 0);
        assert_eq!(same_tick.until.phase(), 1);
    }

    #[test]
    fn tick_waits_resume_in_end_phase() {
        let clock = Clock::new(1000.0);

        let begin = clock.wait_phase(phase::END);
        assert_eq!(begin.until.tick(), 0);
        assert_eq!(begin.until.phase(), phase::END);

        clock.advance_time(ClockTick::new().set_phase(phase::END));

        let delay = clock.wait_ticks(1).unwrap();
        assert_eq!(delay.until.tick(), 1);
        assert_eq!(delay.until.phase(), phase::BEGIN);
    }
}
