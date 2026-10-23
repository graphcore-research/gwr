// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Cache contents, validity, and ownership of parent requests and fills.
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gwr_engine::traits::SimObject;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::entity::Entity;

use super::coherence::CoherenceProtocol;
use super::{CacheConfig, CacheInflightFills, CacheMetrics};
use crate::cache::coherency_manager::{CoherenceOp, CoherenceState};
use crate::memory::CacheHintType;
use crate::memory::traits::{CacheAccess, CacheFillToken};

/// Identity of an original request admitted by this cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ParentId(u64);

/// Cache-local identity of a tracked child request: a read fill, permission
/// acquisition, or nonallocating write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FillId(u64);

#[derive(Clone, Copy, Default)]
enum Validity {
    #[default]
    Missing,
    Valid,
    Pending(FillId),
}

struct Entry {
    tag: Option<u64>,
    validity: Validity,
    permission: CoherenceState,
}

struct Parent<T> {
    request: T,
    pending_fill_ids: BTreeSet<FillId>,
    issued: bool,
    miss: bool,
}

struct Fill {
    waiting_parent_ids: BTreeSet<ParentId>,
    reserved_slot: Option<(usize, usize)>,
    expected_address: u64,
    expected_size_bytes: usize,
    expected_coherence_op: Option<CoherenceOp>,
    expected_access_type: AccessType,
    completion_permission: CoherenceState,
}

pub(super) enum LineAction<T> {
    Advance(u128),
    Send(T, u128),
    Blocked,
    Writeback(T),
}

pub(super) struct CacheContents<T: SimObject + CacheAccess> {
    entity: Rc<Entity>,
    pub(super) config: CacheConfig,
    pub(super) coherence: Option<Rc<dyn CoherenceProtocol<T>>>,
    pub(super) barrier_response: Option<T>,
    /// Deferred invalidations keyed by aligned physical line address.
    pending_invalidations: BTreeMap<u64, T>,
    active_barrier: Option<T>,
    returning: usize,
    sets: Vec<Vec<Entry>>,
    victims: Vec<usize>,
    parents: BTreeMap<ParentId, Parent<T>>,
    fills: BTreeMap<FillId, Fill>,
    owner: Rc<()>,
    peak_inflight_fills: usize,
    next_parent_id_ordinal: u64,
    /// Shared token sequence for tracked children, writebacks, and barriers.
    next_fill_id_ordinal: u64,
    /// Exclusive end tick of the shared client-return service timeline.
    pub(super) response_service_end: u64,
}

impl<T: SimObject + CacheAccess> CacheContents<T> {
    pub(super) fn new(entity: &Rc<Entity>, config: CacheConfig) -> Self {
        let sets = (0..config.num_sets)
            .map(|_| {
                (0..config.num_ways)
                    .map(|_| Entry {
                        tag: None,
                        validity: Validity::Missing,
                        permission: CoherenceState::Invalid,
                    })
                    .collect()
            })
            .collect();
        let victims = vec![0; config.num_sets];
        Self {
            entity: entity.clone(),
            config,
            coherence: None,
            barrier_response: None,
            pending_invalidations: BTreeMap::new(),
            active_barrier: None,
            returning: 0,
            sets,
            victims,
            parents: BTreeMap::new(),
            fills: BTreeMap::new(),
            owner: Rc::new(()),
            peak_inflight_fills: 0,
            next_parent_id_ordinal: 0,
            next_fill_id_ordinal: 0,
            response_service_end: 0,
        }
    }

    pub(super) fn inflight_fills_snapshot(&self) -> CacheInflightFills {
        CacheInflightFills {
            current: self.fills.len(),
            peak: self.peak_inflight_fills,
        }
    }

    pub(super) fn admit_request(&mut self, request: T) -> Result<(ParentId, u128, u128), SimError> {
        let (start, end) = self.validate_parent_request(&request)?;
        let parent_id = ParentId(self.next_parent_id_ordinal);
        self.next_parent_id_ordinal = parent_id
            .0
            .checked_add(1)
            .ok_or_else(|| self.cache_error("cache parent identity exhausted"))?;
        if !self.has_parent_capacity() {
            return Err(self.cache_error("cache parent admission invariant"));
        }
        self.parents.insert(
            parent_id,
            Parent {
                request,
                pending_fill_ids: BTreeSet::new(),
                issued: false,
                miss: false,
            },
        );
        Ok((parent_id, start, end))
    }

    fn validate_parent_request(&self, request: &T) -> Result<(u128, u128), SimError> {
        let supported = match request.access_type() {
            AccessType::ReadRequest => true,
            AccessType::WriteRequest | AccessType::WriteNonPostedRequest => {
                self.coherence.is_some()
            }
            _ => false,
        };
        if !supported {
            return Err(self.cache_error("cache admission requires a supported read/write request"));
        }
        let end =
            self.checked_range_end_exclusive(request.dst_addr(), request.access_size_bytes())?;
        let start = self.line_start_address(request.dst_addr()) as u128;
        // Widened exclusive endpoints represent the legitimate final byte
        // u64::MAX; child packets retain inclusive checked u64 ranges.
        Ok((start, end))
    }

    /// Check or fetch the line at the aligned `current_line_address`.
    /// Line traversal uses `u128` so advancing past the final physical byte
    /// cannot wrap around. Memory fills are bounded separately from parents.
    pub(super) fn process_parent_line(
        &mut self,
        parent_id: ParentId,
        current_line_address: u128,
        _end: u128,
        metrics: &mut CacheMetrics,
    ) -> Result<LineAction<T>, SimError> {
        if self.coherence.is_some() {
            return self.process_coherent_parent_line(parent_id, current_line_address, metrics);
        }
        let address = u64::try_from(current_line_address)
            .map_err(|_| self.cache_error("cache line address outside address space"))?;
        let (set, tag) = self.set_and_tag_for_address(address);
        let validity = self
            .find_way_for_tag(set, tag)
            .map_or(Validity::Missing, |way| self.sets[set][way].validity);
        let next = current_line_address + self.config.line_size_bytes as u128;
        match validity {
            Validity::Valid => {
                metrics.line_hits += 1;
                Ok(LineAction::Advance(next))
            }
            Validity::Pending(fill_id)
                if self.parents[&parent_id].request.cache_hint() == CacheHintType::Allocate =>
            {
                self.coalesce_read_fill(parent_id, fill_id, metrics)?;
                Ok(LineAction::Advance(next))
            }
            _ => self.start_read_fill(parent_id, address, metrics),
        }
    }

    fn coalesce_read_fill(
        &mut self,
        parent_id: ParentId,
        fill_id: FillId,
        metrics: &mut CacheMetrics,
    ) -> SimResult {
        let parent = self.parents.get_mut(&parent_id).unwrap();
        parent.miss = true;
        parent.pending_fill_ids.insert(fill_id);
        let Some(fill) = self.fills.get_mut(&fill_id) else {
            return Err(self.cache_error("cache pending line lost fill"));
        };
        fill.waiting_parent_ids.insert(parent_id);
        metrics.coalesced_lines += 1;
        Ok(())
    }

    fn start_read_fill(
        &mut self,
        parent_id: ParentId,
        address: u64,
        metrics: &mut CacheMetrics,
    ) -> Result<LineAction<T>, SimError> {
        if self.fills.len() >= self.config.max_inflight_fills {
            return Ok(LineAction::Blocked);
        }
        let bytes = self.config.line_size_bytes;
        let next = self.checked_range_end_exclusive(address, bytes)?;
        let fill_id = FillId(self.next_fill_id_ordinal);
        let next_fill_id_ordinal = fill_id
            .0
            .checked_add(1)
            .ok_or_else(|| self.cache_error("cache fill identity exhausted"))?;
        let request = self.parents[&parent_id]
            .request
            .cache_read_fragment(
                address,
                bytes,
                CacheFillToken {
                    owner: self.owner.clone(),
                    ordinal: fill_id.0,
                },
            )
            .map_err(|error| self.cache_error(error))?;
        // Construct the checked packet before changing fill or validity state.
        self.next_fill_id_ordinal = next_fill_id_ordinal;
        self.track_read_fill(parent_id, fill_id, address, &request, metrics)?;
        Ok(LineAction::Send(request, next))
    }

    fn track_read_fill(
        &mut self,
        parent_id: ParentId,
        fill_id: FillId,
        address: u64,
        request: &T,
        metrics: &mut CacheMetrics,
    ) -> SimResult {
        let reserved_slot =
            if self.parents[&parent_id].request.cache_hint() == CacheHintType::Allocate {
                let (set, tag) = self.set_and_tag_for_address(address);
                let way = self.reserve_line(set, tag)?;
                self.sets[set][way].validity = Validity::Pending(fill_id);
                Some((set, way))
            } else {
                None
            };
        let bytes = self.config.line_size_bytes;
        self.fills.insert(
            fill_id,
            Fill {
                waiting_parent_ids: BTreeSet::from([parent_id]),
                reserved_slot,
                expected_address: address,
                expected_size_bytes: bytes,
                expected_coherence_op: None,
                expected_access_type: AccessType::ReadResponse,
                completion_permission: CoherenceState::Shared,
            },
        );
        self.peak_inflight_fills = self.peak_inflight_fills.max(self.fills.len());
        let parent = self.parents.get_mut(&parent_id).unwrap();
        parent.pending_fill_ids.insert(fill_id);
        parent.miss = true;
        metrics.line_misses += 1;
        metrics.fill_requests += 1;
        metrics.downstream_read_bytes += bytes;
        metrics.downstream_request_header_bytes += request.total_bytes();
        Ok(())
    }

    pub(super) fn finish_issuing_parent(
        &mut self,
        parent_id: ParentId,
        metrics: &mut CacheMetrics,
    ) -> Option<T> {
        let parent = self.parents.get_mut(&parent_id).unwrap();
        parent.issued = true;
        if parent.miss {
            metrics.num_misses += 1;
        } else {
            metrics.num_hits += 1;
        }
        self.take_ready_parent(parent_id)
    }

    pub(super) fn complete_child_request(&mut self, response: &T) -> Result<Vec<T>, SimError> {
        // Validate the entire ownership transition before releasing capacity
        // or changing slots, validity, or parent dependencies.
        let fill_id = self.validate_child_response(response)?;
        let fill = self.fills.remove(&fill_id).unwrap();
        self.install_completed_fill(fill_id, &fill, response);
        Ok(self.release_fill_waiters(fill_id, fill.waiting_parent_ids))
    }

    fn validate_child_response(&self, response: &T) -> Result<FillId, SimError> {
        let fill_token = response
            .cache_fill_token()
            .ok_or_else(|| self.cache_error("cache response lacks internal fill token"))?;
        if !Rc::ptr_eq(&fill_token.owner, &self.owner) {
            return Err(self.cache_error("cache response belongs to another fill owner"));
        }
        let fill_id = FillId(fill_token.ordinal);
        let fill = self
            .fills
            .get(&fill_id)
            .ok_or_else(|| self.cache_error("unknown/duplicate cache fill response"))?;
        self.validate_fill_response(fill, response)?;
        self.validate_fill_dependencies(fill_id, fill)?;
        Ok(fill_id)
    }

    fn validate_fill_response(&self, fill: &Fill, response: &T) -> SimResult {
        if response.dst_addr() != fill.expected_address
            || response.access_size_bytes() != fill.expected_size_bytes
        {
            return Err(self.cache_error("cache fill response range mismatch"));
        }
        if response.access_type() != fill.expected_access_type {
            return Err(self.cache_error("cache child response type mismatch"));
        }
        if let Some(protocol) = &self.coherence {
            let op = protocol.coherence_op(response);
            if op != fill.expected_coherence_op
                && !(fill.expected_coherence_op == Some(CoherenceOp::GrantShared)
                    && op == Some(CoherenceOp::GrantExclusive))
            {
                return Err(self.cache_error("cache child response grant mismatch"));
            }
        }
        Ok(())
    }

    fn validate_fill_dependencies(&self, fill_id: FillId, fill: &Fill) -> SimResult {
        for parent_id in &fill.waiting_parent_ids {
            let parent = self
                .parents
                .get(parent_id)
                .ok_or_else(|| self.cache_error("cache fill lost parent"))?;
            if !parent.pending_fill_ids.contains(&fill_id) {
                return Err(self.cache_error("cache fill parent dependency mismatch"));
            }
        }
        Ok(())
    }

    fn install_completed_fill(&mut self, fill_id: FillId, fill: &Fill, response: &T) {
        let Some((set, way)) = fill.reserved_slot else {
            return;
        };
        let entry = &mut self.sets[set][way];
        // Fill IDs are never reused. A revoked or reused slot must not be
        // installed by an obsolete response.
        if !matches!(entry.validity, Validity::Pending(pending_fill_id) if pending_fill_id == fill_id)
        {
            return;
        }
        entry.validity = Validity::Valid;
        entry.permission = if fill.completion_permission == CoherenceState::Shared
            && self.coherence.as_ref().is_some_and(|protocol| {
                protocol.coherence_op(response) == Some(CoherenceOp::GrantExclusive)
            }) {
            CoherenceState::Exclusive
        } else {
            fill.completion_permission
        };
    }

    fn release_fill_waiters(
        &mut self,
        fill_id: FillId,
        waiting_parent_ids: BTreeSet<ParentId>,
    ) -> Vec<T> {
        let mut ready = Vec::new();
        for parent_id in waiting_parent_ids {
            self.parents
                .get_mut(&parent_id)
                .unwrap()
                .pending_fill_ids
                .remove(&fill_id);
            if let Some(request) = self.take_ready_parent(parent_id) {
                ready.push(request);
            }
        }
        ready
    }

    /// Remove a completed parent while retaining coherent admission until its
    /// response has finished client-return service.
    fn take_ready_parent(&mut self, parent_id: ParentId) -> Option<T> {
        let parent = &self.parents[&parent_id];
        if !parent.issued || !parent.pending_fill_ids.is_empty() {
            return None;
        }
        if self.coherence.is_some() {
            self.returning += 1;
        }
        Some(self.parents.remove(&parent_id).unwrap().request)
    }

    /// Writes invalidate every overlapping line, including pending fills.
    /// Point controls invalidate the entire containing tag line.
    pub(super) fn invalidate_range(&mut self, address: u64, bytes: usize) -> Result<(), SimError> {
        let line_size = self.config.line_size_bytes as u128;
        let mut current_line_address = self.line_start_address(address) as u128;
        let end = if bytes == 0 {
            (current_line_address + line_size).min(u64::MAX as u128 + 1)
        } else {
            self.checked_range_end_exclusive(address, bytes)?
        };
        while current_line_address < end {
            let (set, tag) =
                self.set_and_tag_for_address(u64::try_from(current_line_address).map_err(
                    |_| self.cache_error("cache invalidation line address outside address space"),
                )?);
            if let Some(way) = self.find_way_for_tag(set, tag) {
                self.sets[set][way].validity = Validity::Missing;
            }
            current_line_address += line_size;
        }
        Ok(())
    }

    pub(super) fn begin_barrier(&mut self, packet: &T) -> Result<Option<T>, SimError> {
        let Some(protocol) = &self.coherence else {
            return Ok(None);
        };
        if self.active_barrier.is_some() {
            return Err(self.cache_error("barrier already active"));
        }
        let barrier_ordinal = self.next_fill_id_ordinal;
        let next_fill_id_ordinal = barrier_ordinal
            .checked_add(1)
            .ok_or_else(|| self.cache_error("cache identity exhausted"))?;
        let forwarded = protocol
            .build_barrier_request(
                packet,
                CacheFillToken {
                    owner: self.owner.clone(),
                    ordinal: barrier_ordinal,
                },
            )
            .map_err(|error| self.cache_error(error))?;
        self.next_fill_id_ordinal = next_fill_id_ordinal;
        self.active_barrier = Some(forwarded.clone());
        Ok(Some(forwarded))
    }

    pub(super) fn complete_barrier(&mut self, packet: T) -> SimResult {
        self.validate_barrier_response(&packet)?;
        self.active_barrier = None;
        self.barrier_response = Some(packet);
        Ok(())
    }

    fn validate_barrier_response(&self, packet: &T) -> SimResult {
        let expected_request = self
            .active_barrier
            .as_ref()
            .ok_or_else(|| self.cache_error("barrier response has no active barrier"))?;
        let response_fill_token = packet
            .cache_fill_token()
            .ok_or_else(|| self.cache_error("barrier response lacks identity"))?;
        let expected_fill_token = expected_request.cache_fill_token().unwrap();
        if !Rc::ptr_eq(&response_fill_token.owner, &expected_fill_token.owner)
            || response_fill_token.ordinal != expected_fill_token.ordinal
            || packet.src_device() != expected_request.dst_device()
            || packet.dst_device() != expected_request.src_device()
            || packet.dst_addr() != expected_request.dst_addr()
            || packet.access_size_bytes() != expected_request.access_size_bytes()
            || packet.access_type() != AccessType::BarrierResponse
        {
            return Err(self.cache_error("barrier response mismatch"));
        }
        Ok(())
    }

    pub(super) fn is_idle(&self) -> bool {
        self.parents.is_empty()
            && self.fills.is_empty()
            && self.pending_invalidations.is_empty()
            && self.returning == 0
    }

    pub(super) fn complete_parent_return(&mut self) {
        if self.coherence.is_some() {
            self.returning = self
                .returning
                .checked_sub(1)
                .expect("coherent return must own admission");
        }
    }

    pub(super) fn has_parent_capacity(&self) -> bool {
        self.parents.len() + self.returning < self.config.max_inflight_parents
    }

    pub(super) fn checked_range_end_exclusive(
        &self,
        address: u64,
        bytes: usize,
    ) -> Result<u128, SimError> {
        if bytes == 0 {
            return Err(self.cache_error("zero-size cache read/write"));
        }
        let end = address as u128 + bytes as u128;
        if end > u64::MAX as u128 + 1 {
            return Err(self.cache_error("cache address range overflows"));
        }
        Ok(end)
    }

    fn reserve_line(&mut self, set: usize, tag: u64) -> Result<usize, SimError> {
        if let Some(way) = self.find_way_for_tag(set, tag) {
            return Ok(way);
        }
        let way = self.victims[set];
        self.victims[set] = (way + 1) % self.config.num_ways;
        let entry = &mut self.sets[set][way];
        entry.tag = Some(tag);
        entry.validity = Validity::Missing;
        Ok(way)
    }

    fn find_way_for_tag(&self, set: usize, tag: u64) -> Option<usize> {
        self.sets[set].iter().position(|e| e.tag == Some(tag))
    }

    fn line_start_address(&self, address: u64) -> u64 {
        let line_size = self.config.line_size_bytes as u64;
        address - address % line_size
    }

    fn set_and_tag_for_address(&self, address: u64) -> (usize, u64) {
        let line = address / self.config.line_size_bytes as u64;
        (
            (line % self.config.num_sets as u64) as usize,
            line / self.config.num_sets as u64,
        )
    }

    fn cache_error(&self, message: impl std::fmt::Display) -> SimError {
        SimError(format!("{}: {message}", self.entity))
    }
}

mod coherent;

#[cfg(test)]
mod tests;
