// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use futures::future::select_all;
use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::{ComputeModel, EngineId};

/// One nonpreemptive scheduler per PE.
#[derive(Clone)]
pub struct ComputeScheduler {
    state: Rc<RefCell<State>>,
}

impl ComputeScheduler {
    /// Create independent arbitration state for a PE's immutable hardware
    /// model.
    #[must_use]
    pub fn new(model: &Rc<ComputeModel>) -> Self {
        Self {
            state: Rc::new(RefCell::new(State {
                used: vec![false; model.engine_names().len()],
                capacities: model.matrix_accumulator_capacities.clone(),
                current_bytes: vec![0; model.engine_names().len()],
                retained_bytes: vec![0; model.engine_names().len()],
                retained_requests: BTreeMap::new(),
                retained_live: BTreeMap::new(),
                peak_bytes: vec![0; model.engine_names().len()],
                next_id: 0,
                requests: BTreeMap::new(),
            })),
        }
    }

    /// Wait for the first available eligible engine. Pending requests for the
    /// other engines are cancelled when one is granted.
    pub async fn acquire_any(&self, engines: &[EngineId]) -> Result<ComputeReservation, SimError> {
        if engines.is_empty() {
            return sim_error!("no eligible compute engines");
        }
        let (reservation, _, pending) = select_all(
            engines
                .iter()
                .map(|engine| self.acquire(*engine))
                .collect::<Vec<_>>(),
        )
        .await;
        drop(pending);
        reservation
    }
    /// Wait for an exclusive engine whose optional logical accumulator budget
    /// can hold this batch. Invalid rosters reject atomically before enqueue;
    /// capacity-ineligible alternatives are filtered independently.
    pub async fn acquire_any_with_accumulator(
        &self,
        engines: &[EngineId],
        bytes: u64,
    ) -> Result<ComputeReservation, SimError> {
        let eligible = {
            let state = self.state.borrow();
            if engines.is_empty() {
                return sim_error!("no eligible compute engines");
            }
            let mut seen = BTreeSet::new();
            for engine in engines {
                if engine.0 >= state.used.len() {
                    return sim_error!("unknown compute engine ID");
                }
                if !seen.insert(engine.0) {
                    return sim_error!("duplicate compute engine ID");
                }
            }
            engines
                .iter()
                .copied()
                .filter(|engine| {
                    state.capacities[engine.0].is_none_or(|capacity| bytes <= capacity)
                })
                .collect::<Vec<_>>()
        };
        if eligible.is_empty() {
            return sim_error!("matrix accumulator capacity cannot hold batch");
        }
        let (reservation, _, pending) = select_all(
            eligible
                .iter()
                .map(|engine| self.acquire_with_accumulator(*engine, bytes))
                .collect::<Vec<_>>(),
        )
        .await;
        drop(pending);
        reservation
    }

    fn acquire_with_accumulator(&self, engine: EngineId, bytes: u64) -> AcquireBatch {
        AcquireBatch {
            state: self.state.clone(),
            engine,
            bytes,
            id: None,
            done: false,
        }
    }
    #[cfg(test)]
    pub(crate) fn retained_state_for_test(&self) -> (Vec<u64>, Vec<u64>) {
        let s = self.state.borrow();
        (s.retained_bytes.clone(), s.peak_bytes.clone())
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
    /// Request exclusive use of an engine for one batch.
    #[must_use = "an acquisition must be awaited"]
    pub fn acquire(&self, engine: EngineId) -> AcquireBatch {
        AcquireBatch {
            state: self.state.clone(),
            engine,
            bytes: 0,
            id: None,
            done: false,
        }
    }
}

/// Cancellation-safe acquisition future. Repeated polling does not enqueue
/// twice.
pub struct AcquireBatch {
    state: Rc<RefCell<State>>,
    engine: EngineId,
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
        if this.engine.0 >= state.used.len() {
            this.done = true;
            return Poll::Ready(sim_error!("unknown compute engine ID"));
        }
        if state.capacities[this.engine.0].is_some_and(|capacity| this.bytes > capacity) {
            this.done = true;
            return Poll::Ready(sim_error!("matrix accumulator capacity cannot hold batch"));
        }
        let id = if let Some(id) = this.id {
            id
        } else {
            let id = state.next_id;
            let Some(next) = id.checked_add(1) else {
                this.done = true;
                return Poll::Ready(sim_error!("compute request ID overflow"));
            };
            state.next_id = next;
            state.requests.insert(
                id,
                Request {
                    engine: this.engine,
                    bytes: this.bytes,
                    granted: false,
                    waker: cx.waker().clone(),
                },
            );
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
        if granted {
            state.requests.remove(&id);
            this.id = None;
            this.done = true;
        }
        drop(state);
        wake_all(wake);
        if granted {
            Poll::Ready(Ok(ComputeReservation {
                state: this.state.clone(),
                engine: this.engine,
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
                && request.granted
            {
                state.release(request.engine);
            }
            let wake = state.admit();
            drop(state);
            wake_all(wake);
        }
    }
}

/// Holds an engine; dropping it releases capacity.
pub struct ComputeReservation {
    state: Rc<RefCell<State>>,
    engine: EngineId,
}
impl ComputeReservation {
    #[must_use]
    pub fn engine(&self) -> EngineId {
        self.engine
    }
}
impl Drop for ComputeReservation {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.release(self.engine);
        let wake = state.admit();
        drop(state);
        wake_all(wake);
    }
}
struct State {
    used: Vec<bool>,
    capacities: Vec<Option<u64>>,
    current_bytes: Vec<u64>,
    retained_bytes: Vec<u64>,
    retained_requests: BTreeMap<u64, RetainRequest>,
    retained_live: BTreeMap<u64, (EngineId, u64)>,
    peak_bytes: Vec<u64>,
    next_id: u64,
    requests: BTreeMap<u64, Request>,
}

impl State {
    fn admit(&mut self) -> Vec<Waker> {
        let mut wake = self.admit_retained();
        for request in self.requests.values_mut() {
            if request.granted {
                continue;
            }
            let engine = request.engine.0;
            if self.used[engine]
                || request
                    .bytes
                    .checked_add(self.retained_bytes[engine])
                    .is_none_or(|sum| self.capacities[engine].is_some_and(|cap| sum > cap))
            {
                // A request waiting for retained storage cannot prevent the
                // lease owner (or its finalisation) from using the idle engine
                // and eventually releasing that storage. The ordered scan
                // still grants the oldest currently capacity-eligible request.
                continue;
            }
            self.used[engine] = true;
            // One exclusive batch: assign its demand, never sum contenders.
            self.current_bytes[engine] = request.bytes;
            self.peak_bytes[engine] =
                self.peak_bytes[engine].max(request.bytes + self.retained_bytes[engine]);
            request.granted = true;
            wake.push(request.waker.clone());
        }
        wake
    }

    fn release(&mut self, engine: EngineId) {
        self.used[engine.0] = false;
        self.current_bytes[engine.0] = 0;
    }
}
struct Request {
    engine: EngineId,
    bytes: u64,
    granted: bool,
    waker: Waker,
}

fn wake_all(wakers: Vec<Waker>) {
    for waker in wakers {
        waker.wake();
    }
}

#[cfg(test)]
mod matrix_accumulator_tests {
    use super::*;
    fn scheduler(capacities: Vec<Option<u64>>) -> ComputeScheduler {
        let n = capacities.len();
        ComputeScheduler {
            state: Rc::new(RefCell::new(State {
                used: vec![false; n],
                capacities,
                current_bytes: vec![0; n],
                retained_bytes: vec![0; n],
                retained_requests: BTreeMap::new(),
                retained_live: BTreeMap::new(),
                peak_bytes: vec![0; n],
                next_id: 0,
                requests: BTreeMap::new(),
            })),
        }
    }
    fn poll<T: Future + Unpin>(future: &mut T) -> Poll<T::Output> {
        Pin::new(future).poll(&mut Context::from_waker(Waker::noop()))
    }
    fn clean(s: &ComputeScheduler) {
        let state = s.state.borrow();
        assert!(state.requests.is_empty());
        assert!(state.used.iter().all(|v| !v));
        assert!(state.current_bytes.iter().all(|v| *v == 0));
    }
    #[test]
    fn mixed_capacity_filters_before_enqueue_and_invalid_rosters_are_atomic() {
        let s = scheduler(vec![Some(128), Some(256), None]);
        for ids in [
            vec![],
            vec![EngineId(0), EngineId(0)],
            vec![EngineId(1), EngineId(99)],
        ] {
            let before = s.state.borrow().next_id;
            let mut f = Box::pin(s.acquire_any_with_accumulator(&ids, 256));
            assert!(matches!(poll(&mut f), Poll::Ready(Err(_))));
            drop(f);
            clean(&s);
            assert_eq!(s.state.borrow().next_id, before);
        }
        let too_small = [EngineId(0)];
        let before = s.state.borrow().next_id;
        let mut rejected = Box::pin(s.acquire_any_with_accumulator(&too_small, 256));
        let Poll::Ready(Err(error)) = poll(&mut rejected) else {
            panic!("no eligible capacity")
        };
        assert!(error.to_string().contains("capacity cannot hold batch"));
        drop(rejected);
        clean(&s);
        assert_eq!(s.state.borrow().next_id, before);
        let ids = [EngineId(0), EngineId(1)];
        let mut f = Box::pin(s.acquire_any_with_accumulator(&ids, 256));
        let Poll::Ready(Ok(guard)) = poll(&mut f) else {
            panic!("eligible candidate suppressed")
        };
        assert_eq!(guard.engine(), EngineId(1));
        assert_eq!(s.state.borrow().current_bytes, vec![0, 256, 0]);
        assert_eq!(s.state.borrow().peak_bytes, vec![0, 256, 0]);
        drop(guard);
        drop(f);
        clean(&s);
        let mut none = s.acquire_with_accumulator(EngineId(2), 512);
        let Poll::Ready(Ok(guard)) = poll(&mut none) else {
            panic!("unbounded alternative")
        };
        assert_eq!(s.state.borrow().current_bytes[2], 512);
        drop(guard);
        clean(&s);
    }
    #[test]
    fn pending_and_granted_unconsumed_drop_release_exact_demand() {
        let s = scheduler(vec![Some(256)]);
        let mut first = s.acquire_with_accumulator(EngineId(0), 256);
        let Poll::Ready(Ok(guard)) = poll(&mut first) else {
            panic!("first")
        };
        let mut pending = s.acquire_with_accumulator(EngineId(0), 128);
        assert!(poll(&mut pending).is_pending());
        drop(pending);
        assert_eq!(s.state.borrow().current_bytes[0], 256);
        let mut alternative = s.acquire_with_accumulator(EngineId(0), 128);
        assert!(poll(&mut alternative).is_pending());
        drop(guard);
        // State grants on release; consumer deliberately never polls again.
        assert!(s.state.borrow().requests.values().next().unwrap().granted);
        assert_eq!(s.state.borrow().current_bytes[0], 128);
        drop(alternative);
        clean(&s);
        assert_eq!(s.state.borrow().peak_bytes[0], 256);
        let mut retry = s.acquire_with_accumulator(EngineId(0), 256);
        let Poll::Ready(Ok(guard)) = poll(&mut retry) else {
            panic!("retry")
        };
        drop(guard);
        clean(&s);
    }
    #[test]
    #[expect(
        clippy::many_single_char_names,
        reason = "Use conventional matrix dimensions and operand names"
    )]
    fn independent_engines_and_scheduler_instances_hold_separate_demands() {
        let a = scheduler(vec![Some(256), Some(512)]);
        let b = scheduler(vec![Some(256)]);
        let mut x = a.acquire_with_accumulator(EngineId(0), 256);
        let mut y = a.acquire_with_accumulator(EngineId(1), 512);
        let mut z = b.acquire_with_accumulator(EngineId(0), 128);
        let Poll::Ready(Ok(x)) = poll(&mut x) else {
            panic!("x")
        };
        let Poll::Ready(Ok(y)) = poll(&mut y) else {
            panic!("y")
        };
        let Poll::Ready(Ok(z)) = poll(&mut z) else {
            panic!("z")
        };
        assert_eq!(a.state.borrow().current_bytes, vec![256, 512]);
        assert_eq!(b.state.borrow().current_bytes, vec![128]);
        drop((x, y, z));
        clean(&a);
        clean(&b);
    }
    #[test]
    fn request_id_overflow_does_not_reserve_and_valid_retry_succeeds() {
        let s = scheduler(vec![Some(256)]);
        s.state.borrow_mut().next_id = u64::MAX;
        let mut f = s.acquire_with_accumulator(EngineId(0), 256);
        let Poll::Ready(Err(error)) = poll(&mut f) else {
            panic!("overflow")
        };
        assert!(error.to_string().contains("request ID overflow"));
        clean(&s);
        assert_eq!(s.state.borrow().peak_bytes[0], 0);
        s.state.borrow_mut().next_id = 0;
        let mut f = s.acquire_with_accumulator(EngineId(0), 256);
        let Poll::Ready(Ok(guard)) = poll(&mut f) else {
            panic!("retry")
        };
        drop(guard);
        clean(&s);
    }
    #[test]
    fn cancel_native_service_wait_releases_slot_and_demand() {
        let mut engine = gwr_engine::engine::Engine::default();
        let clock = engine.default_clock();
        let scheduler = scheduler(vec![Some(256)]);
        let inspect = scheduler.clone();
        engine.spawner().spawn(async move {
            let ids = [EngineId(0)];
            let guard = scheduler.acquire_any_with_accumulator(&ids, 256).await?;
            assert_eq!(scheduler.state.borrow().current_bytes[0], 256);
            let wait_clock = clock.clone();
            let mut service = Box::pin(async move {
                let _guard = guard;
                wait_clock.wait_ticks(10).unwrap().await;
            });
            futures::future::poll_fn(|cx| {
                assert!(service.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert!(clock.tick_now().tick() < 10);
            drop(service);
            clean(&scheduler);
            let retry = scheduler.acquire_any_with_accumulator(&ids, 128).await?;
            assert_eq!(scheduler.state.borrow().current_bytes[0], 128);
            drop(retry);
            clean(&scheduler);
            Ok(())
        });
        engine.run().unwrap();
        clean(&inspect);
        assert_eq!(inspect.state.borrow().peak_bytes[0], 256);
    }
}

struct RetainRequest {
    engine: EngineId,
    bytes: u64,
    granted: bool,
    waker: Waker,
}
struct RetainedOwner {
    state: Rc<RefCell<State>>,
    engine: EngineId,
    id: u64,
    bytes: u64,
}
impl Drop for RetainedOwner {
    fn drop(&mut self) {
        let mut s = self.state.borrow_mut();
        s.retained_live.remove(&self.id);
        s.retained_bytes[self.engine.0] -= self.bytes;
        let wake = s.admit();
        drop(s);
        wake_all(wake);
    }
}
/// Noncloneable source accumulator lifetime. Private owner/identity cannot be
/// forged; compute reservations hold the owner until their batch is released.
/// ```compile_fail
/// use gwr_models::processing_element::compute::AccumulatorLease;
/// fn forge(lease:AccumulatorLease){let _=lease.owner;}
/// ```
pub struct AccumulatorLease {
    owner: Rc<RetainedOwner>,
}
impl AccumulatorLease {
    #[must_use]
    pub fn engine(&self) -> EngineId {
        self.owner.engine
    }
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.owner.bytes
    }
}
pub struct AcquireAccumulator {
    state: Rc<RefCell<State>>,
    engine: EngineId,
    bytes: u64,
    id: Option<u64>,
    done: bool,
}
impl Future for AcquireAccumulator {
    type Output = Result<AccumulatorLease, SimError>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let x = self.get_mut();
        assert!(!x.done);
        let mut s = x.state.borrow_mut();
        if x.engine.0 >= s.used.len()
            || x.bytes == 0
            || s.capacities[x.engine.0].is_some_and(|cap| x.bytes > cap)
        {
            x.done = true;
            return Poll::Ready(sim_error!("invalid retained accumulator demand"));
        }
        let id = if let Some(id) = x.id {
            id
        } else {
            let id = s.next_id;
            let Some(next) = id.checked_add(1) else {
                x.done = true;
                return Poll::Ready(sim_error!("retained ID overflow"));
            };
            s.next_id = next;
            s.retained_requests.insert(
                id,
                RetainRequest {
                    engine: x.engine,
                    bytes: x.bytes,
                    granted: false,
                    waker: cx.waker().clone(),
                },
            );
            x.id = Some(id);
            id
        };
        s.retained_requests
            .get_mut(&id)
            .unwrap()
            .waker
            .clone_from(cx.waker());
        let wake = s.admit();
        let granted = s.retained_requests[&id].granted;
        if granted {
            s.retained_requests.remove(&id);
            x.id = None;
            x.done = true;
        }
        drop(s);
        wake_all(wake);
        if granted {
            Poll::Ready(Ok(AccumulatorLease {
                owner: Rc::new(RetainedOwner {
                    state: x.state.clone(),
                    engine: x.engine,
                    id,
                    bytes: x.bytes,
                }),
            }))
        } else {
            Poll::Pending
        }
    }
}
impl Drop for AcquireAccumulator {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            let mut s = self.state.borrow_mut();
            if let Some(r) = s.retained_requests.remove(&id)
                && r.granted
            {
                s.retained_live.remove(&id);
                s.retained_bytes[r.engine.0] -= r.bytes;
            }
            let wake = s.admit();
            drop(s);
            wake_all(wake);
        }
    }
}
/// Execution reservation holding the same retained storage, without a second
/// accumulator charge. Dropping the outer lease during a batch cannot reuse it.
pub struct RetainedComputeReservation {
    batch: ComputeReservation,
    _owner: Rc<RetainedOwner>,
}
impl RetainedComputeReservation {
    #[must_use]
    pub fn engine(&self) -> EngineId {
        self.batch.engine()
    }
}
impl ComputeScheduler {
    #[must_use]
    pub fn retain_accumulator(&self, engine: EngineId, bytes: u64) -> AcquireAccumulator {
        AcquireAccumulator {
            state: self.state.clone(),
            engine,
            bytes,
            id: None,
            done: false,
        }
    }
    /// Reserve compute while retaining the validated accumulator owner.
    ///
    /// The returned future owns its lifetime guard, so callers can release a
    /// `RefCell` borrow of the source lease before awaiting admission.
    pub fn acquire_with_retained_accumulator(
        &self,
        lease: &AccumulatorLease,
        batch: &super::EngineBatch,
    ) -> impl Future<Output = Result<RetainedComputeReservation, SimError>> + use<> {
        let valid = {
            let state = self.state.borrow();
            Rc::ptr_eq(&self.state, &lease.owner.state)
                && state.retained_live.get(&lease.owner.id)
                    == Some(&(lease.owner.engine, lease.owner.bytes))
                && batch.matrix_accumulator_bytes <= lease.bytes()
                && batch
                    .alternatives
                    .iter()
                    .any(|(engine, _)| *engine == lease.engine())
        };
        let owner = lease.owner.clone();
        let acquire = self.acquire(lease.engine());
        async move {
            if !valid {
                return sim_error!("foreign/stale/wrong-engine retained accumulator lease");
            }
            Ok(RetainedComputeReservation {
                batch: acquire.await?,
                _owner: owner,
            })
        }
    }
}
impl State {
    fn admit_retained(&mut self) -> Vec<Waker> {
        let mut blocked = BTreeSet::new();
        let mut wake = Vec::new();
        for (id, r) in &mut self.retained_requests {
            if r.granted {
                continue;
            }
            let e = r.engine.0;
            let Some(total) = self.retained_bytes[e]
                .checked_add(self.current_bytes[e])
                .and_then(|v| v.checked_add(r.bytes))
            else {
                blocked.insert(e);
                continue;
            };
            if blocked.contains(&e) || self.capacities[e].is_some_and(|cap| total > cap) {
                blocked.insert(e);
                continue;
            }
            self.retained_bytes[e] += r.bytes;
            self.peak_bytes[e] = self.peak_bytes[e].max(total);
            self.retained_live.insert(*id, (r.engine, r.bytes));
            r.granted = true;
            wake.push(r.waker.clone());
        }
        wake
    }
}

#[cfg(test)]
mod retained_accumulator_tests {
    use super::*;
    fn scheduler(cap: Option<u64>) -> ComputeScheduler {
        ComputeScheduler {
            state: Rc::new(RefCell::new(State {
                used: vec![false, false],
                capacities: vec![cap, cap],
                current_bytes: vec![0, 0],
                retained_bytes: vec![0, 0],
                peak_bytes: vec![0, 0],
                next_id: 0,
                requests: BTreeMap::new(),
                retained_requests: BTreeMap::new(),
                retained_live: BTreeMap::new(),
            })),
        }
    }
    fn poll<T: Future + Unpin>(f: &mut T) -> Poll<T::Output> {
        Pin::new(f).poll(&mut Context::from_waker(Waker::noop()))
    }
    #[test]
    fn retained_compute_and_finalisation_pass_capacity_blocked_ordinary_work() {
        let scheduler = scheduler(Some(8192));
        let mut retain = scheduler.retain_accumulator(EngineId(0), 8192);
        let Poll::Ready(Ok(lease)) = poll(&mut retain) else {
            panic!("retained lease")
        };
        let mut ordinary = scheduler.acquire_with_accumulator(EngineId(0), 8192);
        assert!(poll(&mut ordinary).is_pending());
        let batch = super::super::EngineBatch {
            actions: BTreeMap::default(),
            matrix_accumulator_bytes: 8192,
            engine: EngineId(0),
            alternatives: vec![(EngineId(0), 1)],
            ticks: 1,
            machine_ops: crate::processing_element::MachineOpCounts::default(),
        };
        let mut compute_future =
            Box::pin(scheduler.acquire_with_retained_accumulator(&lease, &batch));
        let Poll::Ready(Ok(compute)) = poll(&mut compute_future) else {
            panic!("idle engine must admit retained-owner progress")
        };
        drop(compute_future);
        assert!(poll(&mut ordinary).is_pending());
        drop(compute);
        // A final conversion can use the same engine without another
        // accumulator allocation, and must also be able to finish the lease.
        let ids = [EngineId(0)];
        let mut finalise = Box::pin(scheduler.acquire_any_with_accumulator(&ids, 0));
        let Poll::Ready(Ok(finalise)) = poll(&mut finalise) else {
            panic!("finalisation must finish retained context")
        };
        assert!(poll(&mut ordinary).is_pending());
        drop(finalise);
        drop(lease);
        let Poll::Ready(Ok(ordinary)) = poll(&mut ordinary) else {
            panic!("ordinary request must run after retained release")
        };
        drop(ordinary);
        assert_eq!(scheduler.state.borrow().retained_bytes, vec![0, 0]);
        assert!(!scheduler.state.borrow().used[0]);
    }

    #[test]
    fn retained_progress_preserves_fifo_among_capacity_eligible_requests() {
        let scheduler = scheduler(Some(8192));
        let mut retain = scheduler.retain_accumulator(EngineId(0), 4096);
        let Poll::Ready(Ok(lease)) = poll(&mut retain) else {
            panic!("retained lease")
        };
        let mut occupy = scheduler.acquire(EngineId(0));
        let Poll::Ready(Ok(occupied)) = poll(&mut occupy) else {
            panic!("initial occupancy")
        };
        let mut older = scheduler.acquire_with_accumulator(EngineId(0), 4096);
        let mut newer = scheduler.acquire(EngineId(0));
        assert!(poll(&mut older).is_pending());
        assert!(poll(&mut newer).is_pending());
        drop(occupied);
        assert!(poll(&mut newer).is_pending());
        let Poll::Ready(Ok(older)) = poll(&mut older) else {
            panic!("oldest eligible request must win")
        };
        drop(older);
        let Poll::Ready(Ok(newer)) = poll(&mut newer) else {
            panic!("next eligible request must follow")
        };
        drop(newer);
        drop(lease);
    }

    #[test]
    fn retained_storage_blocks_contender_not_compute_and_pending_drop_is_atomic() {
        let s = scheduler(Some(8192));
        let mut first = s.retain_accumulator(EngineId(0), 8192);
        let Poll::Ready(Ok(lease)) = poll(&mut first) else {
            panic!()
        };
        assert_eq!(s.state.borrow().retained_bytes, vec![8192, 0]);
        assert!(!s.state.borrow().used[0]);
        let mut second = s.retain_accumulator(EngineId(0), 8192);
        assert!(poll(&mut second).is_pending());
        drop(second);
        assert_eq!(s.state.borrow().retained_requests.len(), 0);
        assert_eq!(s.state.borrow().retained_bytes[0], 8192);
        let mut compute = s.acquire(EngineId(0));
        let Poll::Ready(Ok(batch)) = poll(&mut compute) else {
            panic!()
        };
        drop(batch);
        assert_eq!(s.state.borrow().retained_bytes[0], 8192);
        let mut other = s.acquire(EngineId(1));
        let Poll::Ready(Ok(batch)) = poll(&mut other) else {
            panic!()
        };
        drop(batch);
        drop(lease);
        assert_eq!(s.state.borrow().retained_bytes, vec![0, 0]);
    }
    #[test]
    fn granted_unconsumed_retained_storage_and_too_small_foreign_leases_release_exactly() {
        let s = scheduler(Some(8192));
        let mut first = s.retain_accumulator(EngineId(0), 8192);
        let Poll::Ready(Ok(lease)) = poll(&mut first) else {
            panic!()
        };
        let mut queued = s.retain_accumulator(EngineId(0), 8192);
        assert!(poll(&mut queued).is_pending());
        drop(lease);
        assert_eq!(s.state.borrow().retained_bytes[0], 8192);
        drop(queued);
        assert_eq!(s.state.borrow().retained_bytes[0], 0);
        let too_small = scheduler(Some(8191));
        let mut f = too_small.retain_accumulator(EngineId(0), 8192);
        assert!(matches!(poll(&mut f), Poll::Ready(Err(_))));
        assert_eq!(too_small.state.borrow().next_id, 0);
        let mut f = s.retain_accumulator(EngineId(0), 8192);
        let Poll::Ready(Ok(lease)) = poll(&mut f) else {
            panic!()
        };
        let foreign = scheduler(Some(8192));
        let batch = super::super::EngineBatch {
            actions: BTreeMap::default(),
            matrix_accumulator_bytes: 8192,
            engine: EngineId(0),
            alternatives: vec![(EngineId(0), 1)],
            ticks: 1,
            machine_ops: crate::processing_element::MachineOpCounts::default(),
        };
        let mut invalid = Box::pin(foreign.acquire_with_retained_accumulator(&lease, &batch));
        assert!(matches!(
            invalid
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(_))
        ));
        assert!(foreign.state.borrow().requests.is_empty());
        let mut wrong = batch.clone();
        wrong.alternatives = vec![(EngineId(1), 1)];
        let mut invalid = Box::pin(s.acquire_with_retained_accumulator(&lease, &wrong));
        assert!(matches!(
            invalid
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(_))
        ));
        assert!(s.state.borrow().requests.is_empty());
    }
}
