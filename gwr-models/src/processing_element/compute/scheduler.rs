// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::{ComputeModel, ExecutionUnitId};

/// One nonpreemptive scheduler per PE.
#[derive(Clone)]
pub(in crate::processing_element) struct ComputeScheduler {
    state: Rc<RefCell<ComputeSchedulerState>>,
}

impl ComputeScheduler {
    /// Create independent arbitration state for a PE's immutable hardware
    /// model.
    #[must_use]
    pub(in crate::processing_element) fn new(model: &Rc<ComputeModel>) -> Self {
        Self {
            state: Rc::new(RefCell::new(ComputeSchedulerState {
                used: vec![false; model.execution_unit_names().len()],
                capacities: model.matrix_accumulator_capacities.clone(),
                current_bytes: vec![0; model.execution_unit_names().len()],
                peak_bytes: vec![0; model.execution_unit_names().len()],
                next_id: 0,
                requests: BTreeMap::new(),
            })),
        }
    }

    /// Acquire one unit from prepared alternatives in preference order, with
    /// its service duration. Capacity-ineligible alternatives are filtered
    /// before queuing one logical request.
    #[must_use = "an acquisition must be awaited"]
    pub(in crate::processing_element) fn acquire(
        &self,
        alternatives: &[(ExecutionUnitId, u64)],
        bytes: u64,
    ) -> AcquireBatch {
        AcquireBatch {
            state: self.state.clone(),
            alternatives: alternatives.to_vec(),
            bytes,
            id: None,
            done: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn accumulator_state_for_test(&self) -> (Vec<u64>, Vec<u64>, Vec<bool>, usize) {
        let state = self.state.borrow();
        (
            state.current_bytes.clone(),
            state.peak_bytes.clone(),
            state.used.clone(),
            state.requests.len(),
        )
    }
}

/// Cancellation-safe acquisition future. Repeated polling does not enqueue
/// twice.
pub(in crate::processing_element) struct AcquireBatch {
    state: Rc<RefCell<ComputeSchedulerState>>,
    alternatives: Vec<(ExecutionUnitId, u64)>,
    bytes: u64,
    id: Option<u64>,
    done: bool,
}

impl Future for AcquireBatch {
    type Output = Result<ComputeReservation, SimError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert!(!this.done, "completed compute acquisition polled again");
        let mut state = this.state.borrow_mut();
        let id = if let Some(id) = this.id {
            id
        } else {
            let id = match state.enqueue_request(
                std::mem::take(&mut this.alternatives),
                this.bytes,
                cx.waker(),
            ) {
                Ok(id) => id,
                Err(error) => {
                    this.done = true;
                    return Poll::Ready(Err(error));
                }
            };
            this.id = Some(id);
            id
        };
        state
            .requests
            .get_mut(&id)
            .unwrap()
            .waker
            .clone_from(cx.waker());
        let wake = state.admit();
        let granted = state.requests[&id].granted;
        if granted.is_some() {
            state.requests.remove(&id);
            this.id = None;
            this.done = true;
        }
        drop(state);
        wake_all(wake);
        if let Some((execution_unit, ticks)) = granted {
            Poll::Ready(Ok(ComputeReservation {
                state: this.state.clone(),
                execution_unit,
                ticks,
            }))
        } else {
            Poll::Pending
        }
    }
}

impl Drop for AcquireBatch {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            let mut state = self.state.borrow_mut();
            if let Some(request) = state.requests.remove(&id)
                && let Some((execution_unit, _)) = request.granted
            {
                state.release(execution_unit);
            }
            let wake = state.admit();
            drop(state);
            wake_all(wake);
        }
    }
}

/// Holds an execution unit; dropping it releases capacity.
pub(in crate::processing_element) struct ComputeReservation {
    state: Rc<RefCell<ComputeSchedulerState>>,
    execution_unit: ExecutionUnitId,
    ticks: u64,
}

impl ComputeReservation {
    pub(in crate::processing_element) fn ticks(&self) -> u64 {
        self.ticks
    }

    #[must_use]
    pub(in crate::processing_element) fn execution_unit(&self) -> ExecutionUnitId {
        self.execution_unit
    }
}

impl Drop for ComputeReservation {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.release(self.execution_unit);
        let wake = state.admit();
        drop(state);
        wake_all(wake);
    }
}

struct ComputeSchedulerState {
    used: Vec<bool>,
    capacities: Vec<Option<u64>>,
    current_bytes: Vec<u64>,
    peak_bytes: Vec<u64>,
    next_id: u64,
    requests: BTreeMap<u64, Request>,
}

impl ComputeSchedulerState {
    fn enqueue_request(
        &mut self,
        alternatives: Vec<(ExecutionUnitId, u64)>,
        bytes: u64,
        waker: &Waker,
    ) -> Result<u64, SimError> {
        let alternatives = self.eligible_alternatives(alternatives, bytes)?;
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| SimError("compute request ID overflow".into()))?;
        self.requests.insert(
            id,
            Request {
                alternatives,
                bytes,
                granted: None,
                waker: waker.clone(),
            },
        );
        Ok(id)
    }

    fn eligible_alternatives(
        &self,
        alternatives: Vec<(ExecutionUnitId, u64)>,
        bytes: u64,
    ) -> Result<Vec<(ExecutionUnitId, u64)>, SimError> {
        if alternatives.is_empty() {
            return sim_error!("no eligible execution units");
        }
        let mut seen = BTreeSet::new();
        for (execution_unit, _) in &alternatives {
            if execution_unit.0 >= self.used.len() {
                return sim_error!("unknown execution unit ID");
            }
            if !seen.insert(execution_unit.0) {
                return sim_error!("duplicate execution unit ID");
            }
        }
        let eligible: Vec<_> = alternatives
            .into_iter()
            .filter(|(unit, _)| self.capacities[unit.0].is_none_or(|capacity| bytes <= capacity))
            .collect();
        if eligible.is_empty() {
            return sim_error!("matrix accumulator capacity cannot hold batch");
        }
        Ok(eligible)
    }

    fn admit(&mut self) -> Vec<Waker> {
        let mut wake = Vec::new();
        for request in self.requests.values_mut() {
            if request.granted.is_some() {
                continue;
            }
            // Older requests choose first, but blocked requests do not prevent
            // later requests from using disjoint available units.
            if let Some(&(unit, ticks)) = request
                .alternatives
                .iter()
                .find(|(unit, _)| !self.used[unit.0])
            {
                self.used[unit.0] = true;
                self.current_bytes[unit.0] = request.bytes;
                self.peak_bytes[unit.0] = self.peak_bytes[unit.0].max(request.bytes);
                request.granted = Some((unit, ticks));
                wake.push(request.waker.clone());
            }
        }
        wake
    }

    fn release(&mut self, execution_unit: ExecutionUnitId) {
        self.used[execution_unit.0] = false;
        self.current_bytes[execution_unit.0] = 0;
    }
}

struct Request {
    alternatives: Vec<(ExecutionUnitId, u64)>,
    bytes: u64,
    granted: Option<(ExecutionUnitId, u64)>,
    waker: Waker,
}

fn wake_all(wakers: Vec<Waker>) {
    for waker in wakers {
        waker.wake();
    }
}

#[cfg(test)]
mod matrix_accumulator_tests {
    use futures::future::poll_fn;
    use gwr_engine::engine::Engine;

    use super::*;

    fn assert_scheduler_resources_released(s: &ComputeScheduler) {
        let state = s.state.borrow();
        assert!(state.requests.is_empty());
        assert!(state.used.iter().all(|v| !v));
        assert!(state.current_bytes.iter().all(|v| *v == 0));
    }

    fn assert_execution_unit_list_is_rejected(ids: &[ExecutionUnitId], expected_error: &str) {
        // All valid IDs fit the demand, so capacity cannot mask roster
        // validation.
        let s = scheduler_with_accumulator_capacities(vec![Some(256), Some(256), None]);
        let alternatives: Vec<_> = ids.iter().map(|id| (*id, 1)).collect();
        let mut request = Box::pin(s.acquire(&alternatives, 256));
        let Poll::Ready(Err(error)) = poll_future_once(&mut request) else {
            panic!("invalid execution unit list must be rejected")
        };
        assert!(error.to_string().contains(expected_error), "{error}");
    }

    fn scheduler_with_accumulator_capacities(capacities: Vec<Option<u64>>) -> ComputeScheduler {
        let n = capacities.len();
        ComputeScheduler {
            state: Rc::new(RefCell::new(ComputeSchedulerState {
                used: vec![false; n],
                capacities,
                current_bytes: vec![0; n],
                peak_bytes: vec![0; n],
                next_id: 0,
                requests: BTreeMap::new(),
            })),
        }
    }

    fn poll_future_once<T: Future + Unpin>(future: &mut T) -> Poll<T::Output> {
        Pin::new(future).poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn empty_execution_unit_list_is_rejected() {
        assert_execution_unit_list_is_rejected(&[], "no eligible execution units");
    }

    #[test]
    fn duplicate_execution_unit_ids_are_rejected() {
        assert_execution_unit_list_is_rejected(
            &[ExecutionUnitId(0), ExecutionUnitId(0)],
            "duplicate execution unit ID",
        );
    }

    #[test]
    fn unknown_execution_unit_ids_are_rejected() {
        assert_execution_unit_list_is_rejected(
            &[ExecutionUnitId(1), ExecutionUnitId(99)],
            "unknown execution unit ID",
        );
    }

    #[test]
    fn no_fitting_accumulator_candidate_is_rejected() {
        let s = scheduler_with_accumulator_capacities(vec![Some(128), Some(256), None]);
        let too_small = [(ExecutionUnitId(0), 1)];
        let mut rejected = Box::pin(s.acquire(&too_small, 256));
        let Poll::Ready(Err(error)) = poll_future_once(&mut rejected) else {
            panic!("no eligible capacity")
        };
        assert!(error.to_string().contains("capacity cannot hold batch"));
    }

    #[test]
    fn a_too_small_candidate_does_not_suppress_a_fitting_candidate() {
        let s = scheduler_with_accumulator_capacities(vec![Some(128), Some(256), None]);
        let ids = [(ExecutionUnitId(0), 1), (ExecutionUnitId(1), 1)];
        let mut request = Box::pin(s.acquire(&ids, 256));
        let Poll::Ready(Ok(guard)) = poll_future_once(&mut request) else {
            panic!("eligible candidate suppressed")
        };
        assert_eq!(guard.execution_unit(), ExecutionUnitId(1));
        assert_eq!(s.state.borrow().current_bytes, vec![0, 256, 0]);
        assert_eq!(s.state.borrow().peak_bytes, vec![0, 256, 0]);
        drop(guard);
        drop(request);
        assert_scheduler_resources_released(&s);
    }

    #[test]
    fn unbounded_execution_unit_admits_accumulator_demand() {
        let s = scheduler_with_accumulator_capacities(vec![Some(128), Some(256), None]);
        let mut request = s.acquire(&[(ExecutionUnitId(2), 1)], 512);
        let Poll::Ready(Ok(guard)) = poll_future_once(&mut request) else {
            panic!("unbounded alternative")
        };
        assert_eq!(s.state.borrow().current_bytes[2], 512);
        drop(guard);
        assert_scheduler_resources_released(&s);
    }

    #[test]
    fn dropping_a_pending_request_preserves_the_active_demand() {
        let s = scheduler_with_accumulator_capacities(vec![Some(256)]);
        let mut first = s.acquire(&[(ExecutionUnitId(0), 1)], 256);
        let Poll::Ready(Ok(guard)) = poll_future_once(&mut first) else {
            panic!("first")
        };
        let mut pending = s.acquire(&[(ExecutionUnitId(0), 1)], 128);
        assert!(poll_future_once(&mut pending).is_pending());
        drop(pending);
        assert_eq!(s.state.borrow().current_bytes[0], 256);
        drop(guard);
        assert_scheduler_resources_released(&s);
    }

    #[test]
    fn dropping_a_granted_unconsumed_request_releases_its_demand() {
        let s = scheduler_with_accumulator_capacities(vec![Some(256)]);
        let mut first = s.acquire(&[(ExecutionUnitId(0), 1)], 256);
        let Poll::Ready(Ok(guard)) = poll_future_once(&mut first) else {
            panic!("first")
        };
        let mut alternative = s.acquire(&[(ExecutionUnitId(0), 1)], 128);
        assert!(poll_future_once(&mut alternative).is_pending());
        drop(guard);
        // Releasing the execution unit grants the waiting request; its consumer
        // never polls again.
        assert!(
            s.state
                .borrow()
                .requests
                .values()
                .next()
                .unwrap()
                .granted
                .is_some()
        );
        assert_eq!(s.state.borrow().current_bytes[0], 128);
        drop(alternative);
        assert_scheduler_resources_released(&s);
        assert_eq!(s.state.borrow().peak_bytes[0], 256);
        let mut next_request = s.acquire(&[(ExecutionUnitId(0), 1)], 256);
        let Poll::Ready(Ok(guard)) = poll_future_once(&mut next_request) else {
            panic!("released execution unit must accept another request")
        };
        drop(guard);
        assert_scheduler_resources_released(&s);
    }

    #[test]
    fn independent_execution_units_hold_separate_demands() {
        let s = scheduler_with_accumulator_capacities(vec![Some(256), Some(512)]);
        let mut first = s.acquire(&[(ExecutionUnitId(0), 1)], 256);
        let mut second = s.acquire(&[(ExecutionUnitId(1), 1)], 512);
        let Poll::Ready(Ok(first)) = poll_future_once(&mut first) else {
            panic!("first")
        };
        let Poll::Ready(Ok(second)) = poll_future_once(&mut second) else {
            panic!("second")
        };
        assert_eq!(s.state.borrow().current_bytes, vec![256, 512]);
        drop((first, second));
        assert_scheduler_resources_released(&s);
    }

    #[test]
    fn scheduler_instances_hold_separate_demands() {
        let first = scheduler_with_accumulator_capacities(vec![Some(256)]);
        let second = scheduler_with_accumulator_capacities(vec![Some(256)]);
        let mut first_request = first.acquire(&[(ExecutionUnitId(0), 1)], 256);
        let mut second_request = second.acquire(&[(ExecutionUnitId(0), 1)], 128);
        let Poll::Ready(Ok(first_guard)) = poll_future_once(&mut first_request) else {
            panic!("first")
        };
        let Poll::Ready(Ok(second_guard)) = poll_future_once(&mut second_request) else {
            panic!("second")
        };
        assert_eq!(first.state.borrow().current_bytes, vec![256]);
        assert_eq!(second.state.borrow().current_bytes, vec![128]);
        drop((first_guard, second_guard));
        assert_scheduler_resources_released(&first);
        assert_scheduler_resources_released(&second);
    }

    #[test]
    fn request_id_overflow_is_rejected() {
        let s = scheduler_with_accumulator_capacities(vec![Some(256)]);
        s.state.borrow_mut().next_id = u64::MAX;
        let mut request = s.acquire(&[(ExecutionUnitId(0), 1)], 256);
        let Poll::Ready(Err(error)) = poll_future_once(&mut request) else {
            panic!("overflow")
        };
        assert!(error.to_string().contains("request ID overflow"));
    }

    #[test]
    fn cancel_native_service_wait_releases_slot_and_demand() {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let scheduler = scheduler_with_accumulator_capacities(vec![Some(256)]);
        let inspect = scheduler.clone();
        engine.spawner().spawn(async move {
            let ids = [(ExecutionUnitId(0), 1)];
            let guard = scheduler.acquire(&ids, 256).await?;
            assert_eq!(scheduler.state.borrow().current_bytes[0], 256);
            let wait_clock = clock.clone();
            let mut service = Box::pin(async move {
                let _guard = guard;
                wait_clock.wait_ticks(10).unwrap().await;
            });
            poll_fn(|cx| {
                assert!(service.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert!(clock.tick_now().tick() < 10);
            drop(service);
            assert_scheduler_resources_released(&scheduler);
            let next_reservation = scheduler.acquire(&ids, 128).await?;
            assert_eq!(scheduler.state.borrow().current_bytes[0], 128);
            drop(next_reservation);
            assert_scheduler_resources_released(&scheduler);
            Ok(())
        });
        engine.run().unwrap();
        assert_scheduler_resources_released(&inspect);
        assert_eq!(inspect.state.borrow().peak_bytes[0], 256);
    }
}
