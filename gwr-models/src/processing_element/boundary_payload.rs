// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Optional caller-qualified program boundary payload debt. This is not a
//! physical SRAM/VMEM/register traffic or vendor bandwidth model.
use std::cell::RefCell;
use std::rc::Rc;

use gwr_engine::time::clock::Clock;
use gwr_engine::types::{SimError, SimResult};

/// Tile-side direction: input LSU reads become Writes; output LSU writes Reads.
#[derive(Clone, Copy, Debug)]
pub(crate) enum BoundaryPayloadDirection {
    Read,
    Write,
}
/// Cumulative reserved/settled packed payload; no HBM utilization inference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BoundaryPayloadSnapshot {
    pub admitted_read_bytes: usize,
    pub admitted_write_bytes: usize,
    pub completed_read_bytes: usize,
    pub completed_write_bytes: usize,
    pub canceled_read_bytes: usize,
    pub canceled_write_bytes: usize,
}
#[derive(Clone, Copy, Default)]
struct State {
    exact_end: u128,
    snapshot: BoundaryPayloadSnapshot,
}
/// One combined-direction exact debt resource per PE, shared by its runners.
pub(crate) struct BoundaryPayloadService {
    rate: usize,
    clock: Clock,
    state: RefCell<State>,
}
impl BoundaryPayloadService {
    pub(crate) fn validate_rate(rate: u64) -> Result<usize, SimError> {
        let rate = usize::try_from(rate)
            .map_err(|_| SimError("boundary payload rate does not fit usize".into()))?;
        if rate == 0 {
            return Err(SimError("boundary payload rate must be positive".into()));
        }
        Ok(rate)
    }
    pub(crate) fn new(rate: u64, clock: &Clock) -> Result<Rc<Self>, SimError> {
        let rate = Self::validate_rate(rate)?;
        if clock.freq_mhz() != 1000.0 {
            return Err(SimError(
                "boundary payload service requires actual1ns ticks".into(),
            ));
        }
        Ok(Rc::new(Self {
            rate,
            clock: clock.clone(),
            state: RefCell::new(State::default()),
        }))
    }
    pub(crate) fn snapshot(&self) -> BoundaryPayloadSnapshot {
        self.state.borrow().snapshot
    }
    /// Checks every debt/deadline/counter before a single admission commit.
    /// Empty payload is a no-op and does not advance debt.
    pub(crate) fn reserve(
        self: &Rc<Self>,
        direction: BoundaryPayloadDirection,
        bytes: usize,
    ) -> Result<BoundaryPayloadReservation, SimError> {
        let now = self.clock.tick_now().tick();
        let mut state = self.state.borrow_mut();
        let mut next = *state;
        let deadline = if bytes == 0 {
            now
        } else {
            let arrival = u128::from(now)
                .checked_mul(self.rate as u128)
                .ok_or_else(|| SimError("boundary arrival debt overflows".into()))?;
            let exact = state
                .exact_end
                .max(arrival)
                .checked_add(bytes as u128)
                .ok_or_else(|| SimError("boundary payload debt overflows".into()))?;
            let tick = (exact - 1) / (self.rate as u128) + 1;
            let deadline = u64::try_from(tick)
                .map_err(|_| SimError("boundary payload deadline exceeds u64".into()))?;
            let counter = match direction {
                BoundaryPayloadDirection::Read => &mut next.snapshot.admitted_read_bytes,
                BoundaryPayloadDirection::Write => &mut next.snapshot.admitted_write_bytes,
            };
            *counter = counter
                .checked_add(bytes)
                .ok_or_else(|| SimError("boundary admitted payload counter overflows".into()))?;
            next.exact_end = exact;
            deadline
        };
        *state = next;
        Ok(BoundaryPayloadReservation {
            service: self.clone(),
            direction,
            bytes,
            deadline,
            settled: false,
        })
    }
}
/// Cancellation settles ownership without deleting accepted shared debt.
/// No queue, spawned worker, LSU slot or extra memory ACK is owned here.
pub(crate) struct BoundaryPayloadReservation {
    service: Rc<BoundaryPayloadService>,
    direction: BoundaryPayloadDirection,
    bytes: usize,
    deadline: u64,
    settled: bool,
}
impl BoundaryPayloadReservation {
    #[cfg(test)]
    pub(crate) fn deadline_tick(&self) -> u64 {
        self.deadline
    }
    pub(crate) async fn complete(mut self) -> SimResult {
        self.service
            .clock
            .wait_ticks(
                self.deadline
                    .saturating_sub(self.service.clock.tick_now().tick()),
            )?
            .await;
        self.settle(false);
        Ok(())
    }
    fn settle(&mut self, canceled: bool) {
        if self.settled {
            return;
        }
        let mut state = self.service.state.borrow_mut();
        let counter = match (self.direction, canceled) {
            (BoundaryPayloadDirection::Read, false) => &mut state.snapshot.completed_read_bytes,
            (BoundaryPayloadDirection::Write, false) => &mut state.snapshot.completed_write_bytes,
            (BoundaryPayloadDirection::Read, true) => &mut state.snapshot.canceled_read_bytes,
            (BoundaryPayloadDirection::Write, true) => &mut state.snapshot.canceled_write_bytes,
        };
        // Each ticket settles once. Subset sums cannot exceed its already
        // checked admitted sum; no fallible operation/allocation in Drop.
        *counter += self.bytes;
        debug_assert!(
            state.snapshot.completed_read_bytes
                <= state.snapshot.admitted_read_bytes - state.snapshot.canceled_read_bytes
        );
        debug_assert!(
            state.snapshot.completed_write_bytes
                <= state.snapshot.admitted_write_bytes - state.snapshot.canceled_write_bytes
        );
        self.settled = true;
    }
}
impl Drop for BoundaryPayloadReservation {
    fn drop(&mut self) {
        self.settle(true);
    }
}

#[cfg(test)]
mod tests {
    use gwr_engine::engine::Engine;

    use super::*;
    #[test]
    fn later_deadline_and_counter_errors_leave_debt_and_snapshot_unchanged() {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let s = BoundaryPayloadService::new(4, &clock).unwrap();
        s.state.borrow_mut().exact_end = u128::MAX;
        let before = *s.state.borrow();
        assert!(s.reserve(BoundaryPayloadDirection::Read, 1).is_err());
        assert_eq!(s.state.borrow().exact_end, before.exact_end);
        assert_eq!(s.snapshot(), before.snapshot);
        s.state.borrow_mut().exact_end = u128::from(u64::MAX) * 4;
        let before = *s.state.borrow();
        assert!(s.reserve(BoundaryPayloadDirection::Read, 1).is_err());
        assert_eq!(s.state.borrow().exact_end, before.exact_end);
        assert_eq!(s.snapshot(), before.snapshot);
        s.state.borrow_mut().exact_end = 0;
        s.state.borrow_mut().snapshot.admitted_read_bytes = usize::MAX;
        let before = *s.state.borrow();
        assert!(s.reserve(BoundaryPayloadDirection::Read, 1).is_err());
        assert_eq!(s.state.borrow().exact_end, before.exact_end);
        assert_eq!(s.snapshot(), before.snapshot);
        // Poisoned opposite counter is unaffected by a valid write retry.
        let retry = s.reserve(BoundaryPayloadDirection::Write, 1).unwrap();
        assert_eq!(retry.deadline_tick(), 1);
        drop(retry);
        assert_eq!(s.snapshot().canceled_write_bytes, 1);
    }
    #[test]
    fn zero_and_clock_guards_preserve_exact_state() {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        assert!(BoundaryPayloadService::new(0, &clock).is_err());
        assert!(BoundaryPayloadService::new(4, &engine.clock_ghz(2.0)).is_err());
        let s = BoundaryPayloadService::new(4, &clock).unwrap();
        let r = s.reserve(BoundaryPayloadDirection::Write, 3).unwrap();
        drop(r);
        let before = *s.state.borrow();
        let zero = s.reserve(BoundaryPayloadDirection::Read, 0).unwrap();
        drop(zero);
        assert_eq!(s.state.borrow().exact_end, before.exact_end);
        assert_eq!(s.snapshot(), before.snapshot);
    }
}
