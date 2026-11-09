// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Arrival-aware analytic payload service with one bounded ownership ledger.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gwr_engine::events::repeated::Repeated;
use gwr_engine::port::OutPort;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, SimObject};
use gwr_engine::types::{SimError, SimResult};

/// Nonphysical metadata limit, independent of bandwidth or memory capacity.
pub const MAX_MEMORY_SERVICE_REQUESTS: usize = 4096;

/// Actual live ownership of the optional payload pipeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryServiceSnapshot {
    /// Accepted FIFO requests not yet in payload service.
    pub queued: usize,
    /// Requests currently in the single payload service worker.
    pub active: usize,
    /// Payload-complete responses retained until actual client acknowledgement.
    pub completed: usize,
    /// All admitted owners, including the publishing response.
    pub total: usize,
    /// Maximum observed total admission occupancy.
    pub peak_total: usize,
    /// Lifetime accepted request count, independent of tracing identifiers.
    pub accepted_requests: u64,
    /// Lifetime actual delivered responses or service-complete posted writes.
    pub retired_requests: u64,
}

/// Cumulative admitted payload in one caller-declared lane.
/// These are byte counters, not completed work or physical utilization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryServiceLaneSnapshot {
    pub payload_bytes_read: usize,
    pub payload_bytes_written: usize,
}

struct Ticket<T> {
    response: Option<T>,
    service_tick: u64,
    publish_tick: u64,
}
struct State<T> {
    queued: VecDeque<Ticket<T>>,
    ready: VecDeque<Ticket<T>>,
    exact_end: u128,
    snapshot: MemoryServiceSnapshot,
    lanes: Option<Vec<MemoryServiceLaneSnapshot>>,
    lane_exact_end: Option<Vec<u128>>,
}
pub(super) struct Service<T> {
    capacity: usize,
    rate: usize,
    delay: usize,
    lane_interleave: Option<u64>,
    lane_rates: Option<Vec<usize>>,
    state: RefCell<State<T>>,
    changed: Repeated<()>,
}

impl<T: SimObject> Service<T> {
    #[cfg(test)]
    pub(super) fn new(capacity: usize, rate: usize, delay: usize) -> Result<Rc<Self>, SimError> {
        Self::new_with_lanes(capacity, rate, delay, None)
    }
    pub(super) fn new_with_lanes(
        capacity: usize,
        rate: usize,
        delay: usize,
        recipe: Option<&(u64, Vec<u64>)>,
    ) -> Result<Rc<Self>, SimError> {
        let lanes = if let Some((interleave, rates)) = recipe {
            validate_lane_config(*interleave, rates, rate)?;
            let mut snapshots = Vec::new();
            snapshots
                .try_reserve_exact(rates.len())
                .map_err(|e| SimError(e.to_string()))?;
            snapshots.resize(rates.len(), MemoryServiceLaneSnapshot::default());
            Some(snapshots)
        } else {
            None
        };
        let (lane_rates, lane_exact_end) = if let Some((_, rates)) = recipe {
            let mut converted = Vec::new();
            let mut debts = Vec::new();
            converted
                .try_reserve_exact(rates.len())
                .map_err(|e| SimError(e.to_string()))?;
            debts
                .try_reserve_exact(rates.len())
                .map_err(|e| SimError(e.to_string()))?;
            for &rate in rates {
                converted.push(usize::try_from(rate).map_err(|e| SimError(e.to_string()))?);
                debts.push(0);
            }
            (Some(converted), Some(debts))
        } else {
            (None, None)
        };
        let mut queued = VecDeque::new();
        let mut ready = VecDeque::new();
        queued
            .try_reserve(capacity)
            .map_err(|e| SimError(e.to_string()))?;
        ready
            .try_reserve(capacity)
            .map_err(|e| SimError(e.to_string()))?;
        Ok(Rc::new(Self {
            capacity,
            rate,
            delay,
            lane_interleave: recipe.map(|r| r.0),
            lane_rates,
            state: RefCell::new(State {
                queued,
                ready,
                exact_end: 0,
                snapshot: MemoryServiceSnapshot::default(),
                lanes,
                lane_exact_end,
            }),
            changed: Repeated::default(),
        }))
    }
    pub(super) fn snapshot(&self) -> MemoryServiceSnapshot {
        self.state.borrow().snapshot
    }
    pub(super) fn lane_snapshot(&self) -> Option<Vec<MemoryServiceLaneSnapshot>> {
        self.state.borrow().lanes.clone()
    }
    pub(super) async fn wait_capacity(&self) {
        while self.state.borrow().snapshot.total == self.capacity {
            self.changed.listen().await;
        }
    }
    /// Every fallible deadline/count check precedes the acceptance commit.
    #[cfg(test)]
    pub(super) fn accept(&self, arrival: u64, bytes: usize, response: Option<T>) -> SimResult {
        self.accept_payload(arrival, bytes, response, 0, 0, false)
    }
    /// Address and role are captured from the validated original request.
    /// Preview all affected lane debt and byte counters before one parent
    /// commit. Omitted lanes retain the previous aggregate cursor exactly.
    #[expect(
        clippy::too_many_lines,
        reason = "Admit payload and completion accounting atomically"
    )]
    pub(super) fn accept_payload(
        &self,
        arrival: u64,
        bytes: usize,
        response: Option<T>,
        base: u64,
        address: u64,
        read: bool,
    ) -> SimResult {
        let mut state = self.state.borrow_mut();
        if state.snapshot.total >= self.capacity {
            return Err(SimError("memory service admission budget exhausted".into()));
        }
        let (exact_end, service_tick, lanes, lane_exact_end) = if let Some(current) = &state.lanes {
            let payloads = payload_by_lane(
                base,
                address,
                bytes,
                self.lane_interleave
                    .expect("lane recipe accompanies snapshots"),
                current.len(),
            )?;
            let rates = self
                .lane_rates
                .as_ref()
                .expect("lane rates accompany snapshots");
            let debts = state
                .lane_exact_end
                .as_ref()
                .expect("lane debt accompanies snapshots");
            let mut next = Vec::new();
            let mut next_debts = Vec::new();
            next.try_reserve_exact(current.len())
                .map_err(|e| SimError(e.to_string()))?;
            next_debts
                .try_reserve_exact(current.len())
                .map_err(|e| SimError(e.to_string()))?;
            let mut completion = arrival;
            for (index, (lane, payload)) in current.iter().zip(payloads).enumerate() {
                let mut candidate = *lane;
                let mut end = debts[index];
                if payload != 0 {
                    let rate = rates[index] as u128;
                    let arrival_units = u128::from(arrival)
                        .checked_mul(rate)
                        .ok_or_else(|| SimError("memory lane arrival units overflow".into()))?;
                    end = end
                        .max(arrival_units)
                        .checked_add(payload as u128)
                        .ok_or_else(|| SimError("memory lane byte debt overflows".into()))?;
                    let rounded = end / rate + u128::from(!end.is_multiple_of(rate));
                    completion = completion.max(
                        u64::try_from(rounded)
                            .map_err(|_| SimError("memory lane service tick overflows".into()))?,
                    );
                    if read {
                        candidate.payload_bytes_read = candidate
                            .payload_bytes_read
                            .checked_add(payload)
                            .ok_or_else(|| SimError("memory lane read counter overflows".into()))?;
                    } else {
                        candidate.payload_bytes_written = candidate
                            .payload_bytes_written
                            .checked_add(payload)
                            .ok_or_else(|| {
                                SimError("memory lane write counter overflows".into())
                            })?;
                    }
                }
                next.push(candidate);
                next_debts.push(end);
            }
            (state.exact_end, completion, Some(next), Some(next_debts))
        } else {
            let rate = self.rate as u128;
            let arrival_units = u128::from(arrival)
                .checked_mul(rate)
                .ok_or_else(|| SimError("memory arrival units overflow".into()))?;
            let end = state
                .exact_end
                .max(arrival_units)
                .checked_add(bytes as u128)
                .ok_or_else(|| SimError("memory byte service deadline overflows".into()))?;
            let rounded = end / rate + u128::from(end % rate != 0);
            let tick = u64::try_from(rounded)
                .map_err(|_| SimError("memory service tick overflows".into()))?;
            (end, tick, None, None)
        };
        let publish_tick = if response.is_some() {
            service_tick
                .checked_add(u64::try_from(self.delay).map_err(|e| SimError(e.to_string()))?)
                .ok_or_else(|| SimError("memory fixed-delay deadline overflows".into()))?
        } else {
            service_tick
        };
        let accepted = state
            .snapshot
            .accepted_requests
            .checked_add(1)
            .ok_or_else(|| SimError("memory acceptance counter overflows".into()))?;
        // All deadlines, counters and fallible allocation precede this commit.
        state.lanes = lanes;
        state.lane_exact_end = lane_exact_end;
        state.queued.push_back(Ticket {
            response,
            service_tick,
            publish_tick,
        });
        state.exact_end = exact_end;
        state.snapshot.accepted_requests = accepted;
        state.snapshot.queued += 1;
        state.snapshot.total += 1;
        state.snapshot.peak_total = state.snapshot.peak_total.max(state.snapshot.total);
        drop(state);
        self.changed.notify();
        Ok(())
    }
    pub(super) async fn run_service(self: Rc<Self>, clock: Clock) -> SimResult {
        loop {
            let ticket = loop {
                let next = {
                    let mut state = self.state.borrow_mut();
                    state.queued.pop_front().inspect(|_| {
                        state.snapshot.queued -= 1;
                        state.snapshot.active += 1;
                    })
                };
                if let Some(next) = next {
                    break next;
                }
                self.changed.listen().await;
            };
            clock
                .wait_ticks(ticket.service_tick.saturating_sub(clock.tick_now().tick()))?
                .await;
            let mut state = self.state.borrow_mut();
            state.snapshot.active -= 1;
            if ticket.response.is_some() {
                state.ready.push_back(ticket);
                state.snapshot.completed += 1;
            } else {
                state.snapshot.total -= 1;
                state.snapshot.retired_requests += 1;
            }
            drop(state);
            self.changed.notify();
        }
    }
    pub(super) async fn run_publisher(
        self: Rc<Self>,
        clock: Clock,
        mut tx: OutPort<T>,
    ) -> SimResult {
        loop {
            let ticket = loop {
                let next = self.state.borrow_mut().ready.pop_front();
                if let Some(next) = next {
                    break next;
                }
                self.changed.listen().await;
            };
            clock
                .wait_ticks(ticket.publish_tick.saturating_sub(clock.tick_now().tick()))?
                .await;
            tx.put(
                ticket
                    .response
                    .expect("only responses enter publication queue"),
            )?
            .await;
            // A moved private ticket retires once, only after the real port
            // ACK.
            let mut state = self.state.borrow_mut();
            state.snapshot.completed -= 1;
            state.snapshot.total -= 1;
            state.snapshot.retired_requests += 1;
            drop(state);
            self.changed.notify();
        }
    }
}

/// Nonphysical bounded lane-vector allocation limit, unrelated to capacity.
const MAX_MEMORY_SERVICE_LANES: usize = 256;

pub(super) fn validate_lane_config(interleave: u64, rates: &[u64], aggregate: usize) -> SimResult {
    validate_lane_geometry(interleave, rates.len())?;
    let mut total = 0usize;
    for &rate in rates {
        let rate = usize::try_from(rate).map_err(|e| SimError(e.to_string()))?;
        if rate == 0 {
            return Err(SimError("memory lane rates must be positive".into()));
        }
        total = total
            .checked_add(rate)
            .ok_or_else(|| SimError("memory lane rate sum overflows".into()))?;
    }
    if total > aggregate {
        return Err(SimError(
            "memory lane rate sum exceeds aggregate bandwidth".into(),
        ));
    }
    Ok(())
}

fn validate_lane_geometry(interleave: u64, count: usize) -> SimResult {
    if !interleave.is_power_of_two() || count == 0 || count > MAX_MEMORY_SERVICE_LANES {
        return Err(SimError(
            "memory lanes require power-of-two stripe and count within1..=256".into(),
        ));
    }
    usize::try_from(interleave).map_err(|e| SimError(e.to_string()))?;
    Ok(())
}

/// Conserved payload bytes per caller-declared relative modulo service lane.
/// Uses bounded lane storage and arithmetic full stripe cycles, never children
/// per byte/stripe. This is not a physical HBM address mapping certificate.
/// Zero accounting is allowed here only; real Memory intake rejects zero.
pub fn payload_by_lane(
    base: u64,
    address: u64,
    bytes: usize,
    interleave: u64,
    count: usize,
) -> Result<Vec<usize>, SimError> {
    validate_lane_geometry(interleave, count)?;
    let start = address
        .checked_sub(base)
        .ok_or_else(|| SimError("memory lane address below base".into()))?;
    if bytes != 0 {
        let size = u64::try_from(bytes).map_err(|e| SimError(e.to_string()))?;
        super::checked_last_address(address, size)
            .ok_or_else(|| SimError("memory lane payload range overflows".into()))?;
    }
    let start = u128::from(start);
    let end = start
        .checked_add(bytes as u128)
        .ok_or_else(|| SimError("memory lane range overflows".into()))?;
    let stripe = u128::from(interleave);
    let period = stripe
        .checked_mul(count as u128)
        .ok_or_else(|| SimError("memory lane period overflows".into()))?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|e| SimError(e.to_string()))?;
    for lane in 0..count {
        let lane_start = stripe * lane as u128;
        let prefix = |position: u128| -> Result<u128, SimError> {
            let complete = (position / period)
                .checked_mul(stripe)
                .ok_or_else(|| SimError("memory lane byte count overflows".into()))?;
            let partial = (position % period).saturating_sub(lane_start).min(stripe);
            complete
                .checked_add(partial)
                .ok_or_else(|| SimError("memory lane byte count overflows".into()))
        };
        let payload = prefix(end)?
            .checked_sub(prefix(start)?)
            .ok_or_else(|| SimError("memory lane count underflows".into()))?;
        result.push(usize::try_from(payload).map_err(|e| SimError(e.to_string()))?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::memory_access::MemoryAccess;
    #[test]
    fn out_of_range_service_tick_does_not_commit_deadline_or_ownership() {
        let service = Service::<MemoryAccess>::new(1, 1, 0).unwrap();
        assert!(service.accept(u64::MAX, 1, None).is_err());
        assert_eq!(service.snapshot(), MemoryServiceSnapshot::default());
        assert_eq!(service.state.borrow().exact_end, 0);
        service.accept(u64::MAX - 1, 1, None).unwrap();
        assert_eq!(
            service.state.borrow().queued.front().unwrap().service_tick,
            u64::MAX
        );
    }
    type Fingerprint = (
        MemoryServiceSnapshot,
        u128,
        Option<Vec<MemoryServiceLaneSnapshot>>,
        Option<Vec<u128>>,
        Vec<(u64, u64)>,
        usize,
    );
    fn fingerprint(service: &Service<MemoryAccess>) -> Fingerprint {
        let s = service.state.borrow();
        (
            s.snapshot,
            s.exact_end,
            s.lanes.clone(),
            s.lane_exact_end.clone(),
            s.queued
                .iter()
                .map(|t| (t.service_tick, t.publish_tick))
                .collect(),
            s.ready.len(),
        )
    }
    fn lane_service(delay: usize) -> Rc<Service<MemoryAccess>> {
        Service::new_with_lanes(2, 2, delay, Some(&(4, vec![1, 1]))).unwrap()
    }
    // These boundary seeds exercise private guards, not physically observed
    // traffic: normal intake already bounds every published service tick.
    #[test]
    fn later_lane_debt_overflow_is_atomic_and_unaffected_lane_retry_progresses() {
        let service = lane_service(0);
        service.state.borrow_mut().lane_exact_end.as_mut().unwrap()[1] = u128::MAX;
        let before = fingerprint(&service);
        let error = service.accept_payload(0, 2, None, 0, 3, true).unwrap_err();
        assert!(error.to_string().contains("debt"));
        assert_eq!(fingerprint(&service), before);
        // Only lane0 touched: poisoned lane1 must neither be rounded nor reset.
        service.accept_payload(0, 1, None, 0, 0, true).unwrap();
        let s = service.state.borrow();
        assert_eq!(s.queued.front().unwrap().service_tick, 1);
        assert_eq!(s.lane_exact_end.as_ref().unwrap(), &vec![1, u128::MAX]);
        assert_eq!(s.lanes.as_ref().unwrap()[0].payload_bytes_read, 1);
        assert_eq!(
            s.lanes.as_ref().unwrap()[1],
            MemoryServiceLaneSnapshot::default()
        );
    }
    #[test]
    fn lane_tick_and_payload_counter_failures_leave_all_candidates_unchanged() {
        for scenario in 0..3 {
            let service = lane_service(0);
            {
                let mut s = service.state.borrow_mut();
                match scenario {
                    0 => s.lane_exact_end.as_mut().unwrap()[1] = u128::from(u64::MAX),
                    1 => s.lanes.as_mut().unwrap()[1].payload_bytes_read = usize::MAX,
                    _ => s.lanes.as_mut().unwrap()[1].payload_bytes_written = usize::MAX,
                }
            }
            let before = fingerprint(&service);
            let error = service
                .accept_payload(0, 2, None, 0, 3, scenario != 2)
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(if scenario == 0 { "tick" } else { "counter" })
            );
            assert_eq!(fingerprint(&service), before);
            service.accept_payload(0, 1, None, 0, 0, true).unwrap();
            assert_eq!(
                service.state.borrow().queued.front().unwrap().service_tick,
                1
            );
            assert_eq!(
                service.state.borrow().lane_exact_end.as_ref().unwrap()[0],
                1
            );
        }
    }
    #[test]
    fn lane_delay_and_parent_counter_failures_do_not_commit_debt() {
        use gwr_engine::types::{AccessType, DeviceId};
        let engine = gwr_engine::engine::Engine::default();
        let response = MemoryAccess::new(
            engine.top(),
            AccessType::ReadResponse,
            1,
            0,
            0,
            DeviceId(0),
            DeviceId(0),
            0,
        );
        let service = lane_service(usize::MAX);
        let before = fingerprint(&service);
        assert!(
            service
                .accept_payload(0, 1, Some(response), 0, 0, true)
                .unwrap_err()
                .to_string()
                .contains("deadline")
        );
        assert_eq!(fingerprint(&service), before);
        // Posted parent omits response delay and proves failed reservation left
        // lane0 at zero, rather than accruing speculative debt.
        service.accept_payload(0, 1, None, 0, 0, true).unwrap();
        assert_eq!(
            service.state.borrow().queued.front().unwrap().service_tick,
            1
        );
        let service = lane_service(0);
        service.state.borrow_mut().snapshot.accepted_requests = u64::MAX;
        let before = fingerprint(&service);
        assert!(
            service
                .accept_payload(0, 1, None, 0, 0, true)
                .unwrap_err()
                .to_string()
                .contains("acceptance")
        );
        assert_eq!(fingerprint(&service), before);
        service.state.borrow_mut().snapshot.accepted_requests = 0;
        service.accept_payload(0, 1, None, 0, 0, true).unwrap();
        assert_eq!(
            service.state.borrow().queued.front().unwrap().service_tick,
            1
        );
    }
    #[test]
    fn exhausted_parent_capacity_preserves_lane_debt_and_valid_retry_deadline() {
        let service =
            Service::<MemoryAccess>::new_with_lanes(1, 2, 0, Some(&(4, vec![1, 1]))).unwrap();
        service.accept_payload(0, 1, None, 0, 0, true).unwrap();
        let before = fingerprint(&service);
        assert!(
            service
                .accept_payload(0, 1, None, 0, 0, true)
                .unwrap_err()
                .to_string()
                .contains("budget")
        );
        assert_eq!(fingerprint(&service), before);
        // Module-local retirement simulation; actual ACK retirement is
        // separately exercised by native ports. Preserve the accepted
        // payload debt1.
        {
            let mut s = service.state.borrow_mut();
            s.queued.clear();
            s.snapshot.queued = 0;
            s.snapshot.total = 0;
        }
        service.accept_payload(0, 1, None, 0, 0, true).unwrap();
        assert_eq!(
            service.state.borrow().queued.front().unwrap().service_tick,
            2
        );
    }
}
