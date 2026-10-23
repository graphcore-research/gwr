// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use super::{
    AccessType, BTreeSet, CacheAccess, CacheContents, CacheFillToken, CacheHintType, CacheMetrics,
    CoherenceOp, CoherenceState, Fill, FillId, LineAction, ParentId, SimError, SimObject, Validity,
};

impl<T: SimObject + CacheAccess> CacheContents<T> {
    pub(in crate::cache) fn complete_deferred_invalidations(&mut self) -> Result<Vec<T>, SimError> {
        let ready = self
            .pending_invalidations
            .keys()
            .copied()
            .filter(|address| {
                let (set, tag) = self.set_and_tag_for_address(*address);
                self.find_way_for_tag(set, tag)
                    .is_none_or(|way| !matches!(self.sets[set][way].validity, Validity::Pending(_)))
            })
            .collect::<Vec<_>>();
        let mut messages = Vec::new();
        for address in ready {
            let packet = self.pending_invalidations.remove(&address).unwrap();
            messages.extend(self.handle_coherence_invalidation(&packet)?);
        }
        Ok(messages)
    }

    pub(in crate::cache) fn handle_coherence_invalidation(
        &mut self,
        packet: &T,
    ) -> Result<Vec<T>, SimError> {
        let protocol = self
            .coherence
            .clone()
            .ok_or_else(|| self.cache_error("coherence not enabled"))?;
        if protocol.coherence_op(packet) != Some(CoherenceOp::Invalidate) {
            return Err(self.cache_error("unsupported coherence control"));
        }
        protocol
            .validate_invalidation(packet, self.config.line_size_bytes)
            .map_err(|error| self.cache_error(error))?;
        let address = self.line_start_address(packet.dst_addr());
        let (set, tag) = self.set_and_tag_for_address(address);
        let mut messages = Vec::new();
        if let Some(way) = self.find_way_for_tag(set, tag) {
            if let Validity::Pending(fill_id) = self.sets[set][way].validity
                && self.fills[&fill_id].expected_coherence_op == Some(CoherenceOp::GrantExclusive)
            {
                if self.pending_invalidations.contains_key(&address) {
                    return Err(self.cache_error("duplicate pending invalidation"));
                }
                self.pending_invalidations.insert(address, packet.clone());
                return Ok(messages);
            }
            if self.sets[set][way].permission == CoherenceState::Modified {
                messages.push(self.build_dirty_line_writeback(packet, address)?);
            }
            self.invalidate_coherent_slot(set, way);
        }
        messages.push(protocol.build_invalidation_ack(packet));
        Ok(messages)
    }

    pub(super) fn process_coherent_parent_line(
        &mut self,
        parent_id: ParentId,
        current_line_address: u128,
        metrics: &mut CacheMetrics,
    ) -> Result<LineAction<T>, SimError> {
        let address = u64::try_from(current_line_address)
            .map_err(|_| self.cache_error("cache line address outside address space"))?;
        let next_line_address =
            self.checked_range_end_exclusive(address, self.config.line_size_bytes)?;
        if let Some(action) =
            self.process_existing_coherent_line(parent_id, address, next_line_address, metrics)?
        {
            return Ok(action);
        }
        self.start_coherent_line_miss(parent_id, address, metrics)
    }

    fn process_existing_coherent_line(
        &mut self,
        parent_id: ParentId,
        address: u64,
        next_line_address: u128,
        metrics: &mut CacheMetrics,
    ) -> Result<Option<LineAction<T>>, SimError> {
        let (set, tag) = self.set_and_tag_for_address(address);
        let Some(way) = self.find_way_for_tag(set, tag) else {
            return Ok(None);
        };
        let request = &self.parents[&parent_id].request;
        let read = request.access_type() == AccessType::ReadRequest;
        let allocate = request.cache_hint() == CacheHintType::Allocate;
        let entry = &self.sets[set][way];
        match entry.validity {
            Validity::Valid
                if read
                    || (allocate
                        && matches!(
                            entry.permission,
                            CoherenceState::Exclusive | CoherenceState::Modified
                        )) =>
            {
                if !read {
                    self.sets[set][way].permission = CoherenceState::Modified;
                }
                metrics.line_hits += 1;
                return Ok(Some(LineAction::Advance(next_line_address)));
            }
            Validity::Pending(fill_id) => {
                let shared_read = read
                    && self.fills[&fill_id].expected_coherence_op == Some(CoherenceOp::GrantShared);
                if shared_read && allocate {
                    self.coalesce_read_fill(parent_id, fill_id, metrics)?;
                    return Ok(Some(LineAction::Advance(next_line_address)));
                }
                if !shared_read {
                    return Ok(Some(LineAction::Blocked));
                }
            }
            _ => {}
        }
        if !allocate && !read && self.sets[set][way].permission == CoherenceState::Modified {
            let request = self.parents[&parent_id].request.clone();
            let writeback = self.writeback_and_invalidate_slot(&request, address, set, way)?;
            return Ok(Some(LineAction::Writeback(writeback)));
        }
        Ok(None)
    }

    fn start_coherent_line_miss(
        &mut self,
        parent_id: ParentId,
        address: u64,
        metrics: &mut CacheMetrics,
    ) -> Result<LineAction<T>, SimError> {
        if self.fills.len() >= self.config.max_inflight_fills {
            return Ok(LineAction::Blocked);
        }
        let request = self.parents[&parent_id].request.clone();
        let install_way = if request.cache_hint() == CacheHintType::Allocate {
            let (set, _) = self.set_and_tag_for_address(address);
            let Some(way) = self.select_coherent_install_way(address) else {
                return Ok(LineAction::Blocked);
            };
            if self.sets[set][way].permission == CoherenceState::Modified {
                let victim_address = self.cached_line_address(set, way)?;
                let writeback =
                    self.writeback_and_invalidate_slot(&request, victim_address, set, way)?;
                return Ok(LineAction::Writeback(writeback));
            }
            Some(way)
        } else {
            None
        };
        self.start_coherent_child_request(parent_id, address, install_way, &request, metrics)
    }

    fn select_coherent_install_way(&self, address: u64) -> Option<usize> {
        let (set, tag) = self.set_and_tag_for_address(address);
        self.find_way_for_tag(set, tag).or_else(|| {
            (0..self.config.num_ways)
                .map(|offset| (self.victims[set] + offset) % self.config.num_ways)
                .find(|way| !matches!(self.sets[set][*way].validity, Validity::Pending(_)))
        })
    }

    fn cached_line_address(&self, set: usize, way: usize) -> Result<u64, SimError> {
        (self.sets[set][way].tag.unwrap() * self.config.num_sets as u64 + set as u64)
            .checked_mul(self.config.line_size_bytes as u64)
            .ok_or_else(|| self.cache_error("victim address overflow"))
    }

    fn writeback_and_invalidate_slot(
        &mut self,
        request: &T,
        address: u64,
        set: usize,
        way: usize,
    ) -> Result<T, SimError> {
        let writeback = self.build_dirty_line_writeback(request, address)?;
        self.invalidate_coherent_slot(set, way);
        Ok(writeback)
    }

    fn build_dirty_line_writeback(&mut self, packet: &T, address: u64) -> Result<T, SimError> {
        let writeback_ordinal = self.next_fill_id_ordinal;
        let next_fill_id_ordinal = writeback_ordinal
            .checked_add(1)
            .ok_or_else(|| self.cache_error("cache child identity exhausted"))?;
        let message = self
            .coherence
            .as_ref()
            .unwrap()
            .build_child_request(
                packet,
                AccessType::WriteRequest,
                address,
                self.config.line_size_bytes,
                CacheFillToken {
                    owner: self.owner.clone(),
                    ordinal: writeback_ordinal,
                },
                Some(CoherenceOp::Writeback),
            )
            .map_err(|error| self.cache_error(error))?;
        self.next_fill_id_ordinal = next_fill_id_ordinal;
        Ok(message)
    }

    fn start_coherent_child_request(
        &mut self,
        parent_id: ParentId,
        address: u64,
        install_way: Option<usize>,
        request: &T,
        metrics: &mut CacheMetrics,
    ) -> Result<LineAction<T>, SimError> {
        let next_line_address =
            self.checked_range_end_exclusive(address, self.config.line_size_bytes)?;
        let fill_id = FillId(self.next_fill_id_ordinal);
        let next_fill_id_ordinal = fill_id
            .0
            .checked_add(1)
            .ok_or_else(|| self.cache_error("cache child identity exhausted"))?;
        let (child_address, child_size_bytes) = self.coherent_child_range(request, address)?;
        let child =
            self.build_coherent_child_request(request, fill_id, child_address, child_size_bytes)?;
        // Build the checked packet before changing identities or cache state.
        self.next_fill_id_ordinal = next_fill_id_ordinal;
        let reserved_slot = self.prepare_coherent_fill_slot(request, address, install_way, fill_id);
        let fill = Self::create_coherent_fill(
            parent_id,
            request,
            child_address,
            child_size_bytes,
            reserved_slot,
        );
        self.track_coherent_fill(parent_id, fill_id, fill);
        Self::record_coherent_miss_metrics(request, &child, child_size_bytes, metrics);
        Ok(LineAction::Send(child, next_line_address))
    }

    fn build_coherent_child_request(
        &self,
        request: &T,
        fill_id: FillId,
        child_address: u64,
        child_size_bytes: usize,
    ) -> Result<T, SimError> {
        let read = request.access_type() == AccessType::ReadRequest;
        let allocate = request.cache_hint() == CacheHintType::Allocate;
        let access_type = if read {
            AccessType::ReadRequest
        } else if allocate {
            AccessType::Control
        } else {
            AccessType::WriteNonPostedRequest
        };
        let coherence_op = if read {
            CoherenceOp::SharedRead
        } else {
            CoherenceOp::ExclusiveWrite
        };
        self.coherence
            .as_ref()
            .unwrap()
            .build_child_request(
                request,
                access_type,
                child_address,
                child_size_bytes,
                CacheFillToken {
                    owner: self.owner.clone(),
                    ordinal: fill_id.0,
                },
                Some(coherence_op),
            )
            .map_err(|error| self.cache_error(error))
    }

    fn coherent_child_range(&self, request: &T, address: u64) -> Result<(u64, usize), SimError> {
        if request.access_type() == AccessType::ReadRequest
            || request.cache_hint() == CacheHintType::Allocate
        {
            return Ok((address, self.config.line_size_bytes));
        }
        let line_end = self.checked_range_end_exclusive(address, self.config.line_size_bytes)?;
        let request_end =
            self.checked_range_end_exclusive(request.dst_addr(), request.access_size_bytes())?;
        let child_start = (address as u128).max(request.dst_addr() as u128);
        let child_end = line_end.min(request_end);
        Ok((child_start as u64, (child_end - child_start) as usize))
    }

    fn prepare_coherent_fill_slot(
        &mut self,
        request: &T,
        address: u64,
        install_way: Option<usize>,
        fill_id: FillId,
    ) -> Option<(usize, usize)> {
        let (set, tag) = self.set_and_tag_for_address(address);
        if let Some(way) = install_way {
            self.sets[set][way].tag = Some(tag);
            self.sets[set][way].validity = Validity::Pending(fill_id);
            self.sets[set][way].permission = CoherenceState::Invalid;
            self.victims[set] = (way + 1) % self.config.num_ways;
            Some((set, way))
        } else {
            if request.access_type() != AccessType::ReadRequest
                && let Some(way) = self.find_way_for_tag(set, tag)
            {
                self.invalidate_coherent_slot(set, way);
            }
            None
        }
    }

    fn create_coherent_fill(
        parent_id: ParentId,
        request: &T,
        child_address: u64,
        child_size_bytes: usize,
        reserved_slot: Option<(usize, usize)>,
    ) -> Fill {
        let read = request.access_type() == AccessType::ReadRequest;
        let allocate = request.cache_hint() == CacheHintType::Allocate;
        Fill {
            waiting_parent_ids: BTreeSet::from([parent_id]),
            reserved_slot,
            expected_address: child_address,
            expected_size_bytes: child_size_bytes,
            expected_coherence_op: if read {
                Some(CoherenceOp::GrantShared)
            } else if allocate {
                Some(CoherenceOp::GrantExclusive)
            } else {
                None
            },
            expected_access_type: if read {
                AccessType::ReadResponse
            } else if allocate {
                AccessType::Control
            } else {
                AccessType::WriteNonPostedResponse
            },
            completion_permission: if read {
                CoherenceState::Shared
            } else {
                CoherenceState::Modified
            },
        }
    }

    fn track_coherent_fill(&mut self, parent_id: ParentId, fill_id: FillId, fill: Fill) {
        self.fills.insert(fill_id, fill);
        self.peak_inflight_fills = self.peak_inflight_fills.max(self.fills.len());
        let parent = self.parents.get_mut(&parent_id).unwrap();
        parent.miss = true;
        parent.pending_fill_ids.insert(fill_id);
    }

    fn record_coherent_miss_metrics(
        request: &T,
        child: &T,
        child_size_bytes: usize,
        metrics: &mut CacheMetrics,
    ) {
        let read = request.access_type() == AccessType::ReadRequest;
        let nonallocating_write = !read && request.cache_hint() != CacheHintType::Allocate;
        metrics.line_misses += 1;
        if read {
            metrics.fill_requests += 1;
            metrics.downstream_read_bytes += child_size_bytes;
        } else if nonallocating_write {
            metrics.downstream_write_bytes += child_size_bytes;
        }
        metrics.downstream_request_header_bytes += child.total_bytes()
            - if nonallocating_write {
                child_size_bytes
            } else {
                0
            };
    }

    fn invalidate_coherent_slot(&mut self, set: usize, way: usize) {
        self.sets[set][way].validity = Validity::Missing;
        self.sets[set][way].permission = CoherenceState::Invalid;
    }
}
