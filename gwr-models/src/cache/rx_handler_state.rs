// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gwr_engine::events::repeated::Repeated;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::SimObject;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::debug;
use gwr_track::entity::Entity;

use super::contents::CacheContents;
use super::line_state::{LineState, LocalWriteModified};
use super::log_cache_forward;
use super::metrics::CacheMetrics;
use crate::cache::coherency_manager::{CoherenceOp, CoherenceState};
use crate::cache::traits::CoherentAccess;

pub(super) enum RetryRequest<T> {
    Device(T),
    Pending(T),
}

pub(super) struct RxHandlingState<T>
where
    T: SimObject + CoherentAccess,
{
    pub(super) entity: Rc<Entity>,
    pub(super) rx: InPort<T>,
    pub(super) clock: Clock,
    pub(super) contents: Rc<RefCell<CacheContents<T>>>,
    pub(super) metrics: Rc<CacheMetrics>,
    pub(super) bw_bytes_per_cycle: usize,
    retry_requests: Rc<RefCell<VecDeque<RetryRequest<T>>>>,
    retry_changed: Repeated<()>,
}

impl<T> RxHandlingState<T>
where
    T: SimObject + CoherentAccess,
{
    pub(super) fn new(
        entity: Rc<Entity>,
        rx: InPort<T>,
        clock: Clock,
        contents: Rc<RefCell<CacheContents<T>>>,
        metrics: Rc<CacheMetrics>,
        retry_requests: Rc<RefCell<VecDeque<RetryRequest<T>>>>,
        retry_changed: Repeated<()>,
    ) -> Self {
        let bw_bytes_per_cycle = contents.borrow().bw_bytes_per_cycle();
        Self {
            entity,
            rx,
            clock,
            contents,
            metrics,
            bw_bytes_per_cycle,
            retry_requests,
            retry_changed,
        }
    }

    pub(super) fn has_retry_requests(&self) -> bool {
        !self.retry_requests.borrow().is_empty()
    }

    pub(super) fn queue_device_retry_request(&self, request: T) {
        self.retry_requests
            .borrow_mut()
            .push_back(RetryRequest::Device(request));
        self.retry_changed.notify();
    }

    pub(super) fn queue_pending_retry_request(&self, request: T) {
        self.retry_requests
            .borrow_mut()
            .push_back(RetryRequest::Pending(request));
        self.retry_changed.notify();
    }

    pub(super) fn take_next_retry_request(&self) -> Option<RetryRequest<T>> {
        self.retry_requests.borrow_mut().pop_front()
    }

    pub(super) fn retry_changed_event(&self) -> Repeated<()> {
        self.retry_changed.clone()
    }

    pub(super) fn record_sent_coherence_op(&self, op: Option<CoherenceOp>) {
        self.metrics.record_sent_coherence_op(op);
    }

    pub(super) fn record_received_coherence_op(&self, op: Option<CoherenceOp>) {
        self.metrics.record_received_coherence_op(op);
    }

    pub(super) fn record_completed_write_payload(&self, request: &T) {
        self.metrics
            .record_completed_write_payload(request.access_type(), request.access_size_bytes());
    }

    pub(super) fn rewrite_request_source(
        &self,
        request: &T,
        coherence_op: Option<CoherenceOp>,
    ) -> Result<T, SimError> {
        let mut forwarded = request.clone();
        if self.contents.borrow().is_coherent() {
            let cache_device_id = self.contents.borrow().device_id();
            let dst_device = self
                .contents
                .borrow()
                .coherency_manager_device_id_for_addr(request.dst_addr())?;
            forwarded = forwarded.with_routing(dst_device, cache_device_id);
        }
        let forwarded = forwarded.with_coherence_op(coherence_op);
        log_cache_forward(&self.entity, "Forward request", request, &forwarded);
        Ok(forwarded)
    }

    pub(super) fn prepare_miss_request(
        &self,
        request: &T,
        requested_state: CoherenceState,
    ) -> Result<T, SimError> {
        let coherence_op = match requested_state {
            CoherenceState::Shared => Some(CoherenceOp::SharedRead),
            CoherenceState::Exclusive | CoherenceState::Modified => {
                Some(CoherenceOp::ExclusiveWrite)
            }
            CoherenceState::Invalid => return sim_error!("Cannot request invalid coherence state"),
        };
        self.rewrite_request_source(request, coherence_op)
    }

    pub(super) async fn queue_dirty_line_writeback(
        &self,
        mem_ack: &mut OutPort<T>,
        request: &T,
        addr: u64,
    ) -> SimResult {
        let writeback = self.make_writeback(request, addr)?;
        debug!(self.entity ; "Write back dirty line 0x{addr:x} as {writeback}");
        mem_ack.put(writeback)?.await;
        Ok(())
    }

    fn make_writeback(&self, request: &T, addr: u64) -> Result<T, SimError> {
        let contents = self.contents.borrow();
        let cache_device_id = contents.device_id();
        let writeback_dst_device = contents.device_id_for_addr(addr)?;
        let writeback = request
            .clone()
            .with_access_type(AccessType::WriteRequest)
            .with_dst_addr(addr)
            .with_routing(writeback_dst_device, cache_device_id)
            .with_coherence_op(None);
        Ok(writeback)
    }

    pub(super) async fn complete_local_write(
        &self,
        rsp: Option<&mut OutPort<T>>,
        request: T,
    ) -> SimResult {
        let addr = request.dst_addr();
        debug!(
            self.entity ;
            "Complete local write access {} for line 0x{:x}; mark modified",
            request.id(),
            addr
        );
        self.contents
            .borrow_mut()
            .transition::<LocalWriteModified>(addr);
        self.record_completed_write_payload(&request);
        if request.access_type() == AccessType::WriteNonPostedRequest {
            let response = request.to_response(self.contents.as_ref())?;
            rsp.expect("non-posted write requires a response port")
                .put(response)?
                .await;
        }
        Ok(())
    }

    fn grant_completes_pending_write(&self, addr: u64) -> bool {
        self.contents.borrow().has_pending_write_for_addr(addr)
    }

    pub(super) async fn apply_grant_exclusive(
        &self,
        rsp: Option<&mut OutPort<T>>,
        access: &T,
    ) -> SimResult {
        let modified = self.grant_completes_pending_write(access.dst_addr());
        let granted_state = if modified {
            LineState::Modified
        } else {
            LineState::Exclusive
        };
        debug!(
            self.entity ;
            "Grant exclusive access {} for line 0x{:x}; set state {}",
            access.id(),
            access.dst_addr(),
            granted_state.as_str()
        );
        self.contents
            .borrow_mut()
            .grant_exclusive(access.dst_addr(), modified);
        if let Some(rsp) = rsp {
            self.complete_pending_non_posted_writes(rsp, access).await?;
        }
        Ok(())
    }

    pub(super) async fn complete_pending_non_posted_writes(
        &self,
        rsp_arb_0: &mut OutPort<T>,
        access: &T,
    ) -> SimResult {
        let maybe_pending = self
            .contents
            .borrow_mut()
            .take_pending_nonposted_completions(access);
        if let Some(pending) = maybe_pending {
            for request in pending {
                if request.access_type() == AccessType::WriteNonPostedRequest {
                    self.record_completed_write_payload(&request);
                    let response = request.to_response(self.contents.as_ref())?;
                    rsp_arb_0.put(response)?.await;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn complete_noallocate_access(
        &self,
        rsp: &mut OutPort<T>,
        access: &T,
    ) -> Result<bool, SimError> {
        let pending = self
            .contents
            .borrow_mut()
            .take_pending_noallocate_completion(access);
        let Some(request) = pending else {
            return Ok(false);
        };

        debug!(
            self.entity ;
            "Complete no-allocate access {} for line 0x{:x} without filling cache",
            request.id(),
            request.dst_addr()
        );
        self.record_completed_write_payload(&request);
        let response = request.to_response(self.contents.as_ref())?;
        rsp.put(response)?.await;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use gwr_engine::test_helpers::start_test;

    use super::super::test_helpers::{test_access, test_config};
    use super::*;
    use crate::memory::memory_map::DeviceId;
    use crate::memory::traits::ReadMemory;

    #[test]
    fn cache_internal_helper_branches_are_covered() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let entity = engine.top().clone();
        let contents = Rc::new(RefCell::new(CacheContents::new(test_config(4, 2))));
        let rx = InPort::new(&engine, &clock, &entity, "rx");
        let state = RxHandlingState::new(
            entity.clone(),
            rx,
            clock.clone(),
            contents.clone(),
            Rc::new(CacheMetrics::default()),
            Rc::new(RefCell::new(VecDeque::new())),
            Repeated::default(),
        );
        let request = test_access(&entity, AccessType::ReadRequest, 0x100, DeviceId(1));

        assert_eq!(contents.borrow().read(), Vec::<u8>::new());
        assert_eq!(contents.read(), Vec::<u8>::new());

        let err = state
            .prepare_miss_request(&request, CoherenceState::Invalid)
            .unwrap_err();
        assert!(err.0.contains("Cannot request invalid coherence state"));
    }
}
