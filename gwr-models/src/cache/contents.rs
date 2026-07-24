// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::mem;
use std::rc::Rc;

use gwr_engine::traits::SimObject;
use gwr_engine::types::{AccessType, SimError};

use super::config::CacheConfig;
use super::line_state::{
    GrantExclusiveClean, GrantExclusiveModified, InvalidateLine, LineState, LineStateTransition,
};
use crate::cache::traits::CoherentAccess;
use crate::memory::memory_map::{DeviceId, MemoryMap};
use crate::memory::traits::ReadMemory;

pub(super) type Tag = u64;
pub(super) type SetIndex = usize;

// Cache structure:
//  A set comprises N-ways
type Set = Vec<CacheEntry>;
//  The cache comprises M-sets
type Sets = Vec<Set>;

pub(super) struct CacheContents<T>
where
    T: SimObject + CoherentAccess,
{
    config: CacheConfig,
    coherency_manager_memory_map: Option<Rc<MemoryMap>>,
    sets: Sets,
    pending_line_waiters: Vec<(Tag, SetIndex, T)>,
    pending_nonposted_completions: Vec<(Tag, SetIndex, T)>,
    pending_noallocate_completions: Vec<T>,
    blocked_device_requests: VecDeque<T>,
    active_barrier: Option<T>,
    barrier_forwarded: bool,
    lru_indices: Vec<usize>,
}

impl<T> CacheContents<T>
where
    T: SimObject + CoherentAccess,
{
    pub(super) fn new(config: CacheConfig) -> Self {
        let sets = vec![vec![CacheEntry::default(); config.num_ways]; config.num_sets];
        let lru_indices = vec![0; config.num_sets];
        let coherency_manager_memory_map = config.coherency_manager_memory_map.clone();
        Self {
            config,
            coherency_manager_memory_map,
            sets,
            pending_line_waiters: Vec::new(),
            pending_nonposted_completions: Vec::new(),
            pending_noallocate_completions: Vec::new(),
            blocked_device_requests: VecDeque::new(),
            active_barrier: None,
            barrier_forwarded: false,
            lru_indices,
        }
    }

    /// Split up an address into its component parts:
    ///
    ///  msb                      lsb
    ///  +-----+-----------+--------+
    ///  | tag | set_index | offset |
    ///  +-----+-----------+--------+
    ///
    /// Where:
    ///  - offset within a cache line
    ///  - set_index is the part of the address used to select a cache set
    ///  - tag contains the rest of the address that is compared to determine
    ///    address matches
    pub(super) fn tag_and_set_index_for_addr(&self, addr: u64) -> (Tag, SetIndex) {
        let set_index = (addr as usize / self.config.line_size_bytes) % self.config.num_sets;
        let tag = addr / self.config.line_size_bytes as u64 / self.config.num_sets as u64;
        (tag, set_index)
    }

    fn addr_for_tag_and_set_index(&self, tag: Tag, set_index: SetIndex) -> u64 {
        ((tag * self.config.num_sets as u64) + set_index as u64)
            * self.config.line_size_bytes as u64
    }

    pub(super) fn state_for(&self, addr: u64) -> Option<LineState> {
        let (tag, set_index) = self.tag_and_set_index_for_addr(addr);
        for i in 0..self.config.num_ways {
            if (self.sets[set_index][i].line_state != LineState::Invalid)
                && self.sets[set_index][i].tag == tag
            {
                return Some(self.sets[set_index][i].line_state);
            }
        }
        None
    }

    pub(super) fn find_way(&self, tag: Tag, set_index: SetIndex) -> Option<usize> {
        (0..self.config.num_ways).find(|&i| {
            self.sets[set_index][i].line_state != LineState::Invalid
                && self.sets[set_index][i].tag == tag
        })
    }

    fn next_evictable_way<TTransition: LineStateTransition>(
        &self,
        set_index: SetIndex,
    ) -> Option<usize> {
        (0..self.config.num_ways)
            .map(|offset| (self.lru_indices[set_index] + offset) % self.config.num_ways)
            .find(|&way| {
                let entry = &self.sets[set_index][way];
                entry.line_state.is_evictable() && entry.can_apply::<TTransition>()
            })
    }

    pub(super) fn allocate<TTransition: LineStateTransition>(
        &mut self,
        addr: u64,
    ) -> AllocateResult {
        let (tag, set_index) = self.tag_and_set_index_for_addr(addr);

        let insert_index = self
            .find_way(tag, set_index)
            .filter(|&way| self.sets[set_index][way].can_apply::<TTransition>())
            .or_else(|| self.next_evictable_way::<TTransition>(set_index));
        let insert_index = if let Some(insert_index) = insert_index {
            insert_index
        } else {
            let victim_index = self.lru_indices[set_index];
            if self.sets[set_index][victim_index].line_state == LineState::Modified {
                let evicted_modified_addr = self
                    .addr_for_tag_and_set_index(self.sets[set_index][victim_index].tag, set_index);
                return AllocateResult::NeedsWriteback {
                    evicted_modified_addr,
                };
            }

            return AllocateResult::Blocked;
        };

        self.lru_indices[set_index] = (insert_index + 1) % self.config.num_ways;

        self.sets[set_index][insert_index].tag = tag;
        self.sets[set_index][insert_index].apply::<TTransition>();
        AllocateResult::Allocated
    }

    pub(super) fn evict_modified(&mut self, addr: u64) -> bool {
        let (tag, set_index) = self.tag_and_set_index_for_addr(addr);

        let Some(way) = self.find_way(tag, set_index) else {
            return false;
        };

        let entry = &mut self.sets[set_index][way];
        if entry.line_state != LineState::Modified {
            return false;
        }

        if entry.apply::<InvalidateLine>() {
            entry.tag = 0;
            return true;
        }

        false
    }

    pub(super) fn transition<TTransition: LineStateTransition>(&mut self, addr: u64) -> bool {
        let (tag, set_index) = self.tag_and_set_index_for_addr(addr);

        for i in 0..self.config.num_ways {
            if (self.sets[set_index][i].line_state != LineState::Invalid)
                && self.sets[set_index][i].tag == tag
            {
                return self.sets[set_index][i].apply::<TTransition>();
            }
        }
        false
    }

    pub(super) fn grant_exclusive(&mut self, addr: u64, modified: bool) -> bool {
        if modified {
            self.transition::<GrantExclusiveModified>(addr)
        } else {
            self.transition::<GrantExclusiveClean>(addr)
        }
    }

    pub(super) fn is_modified(&self, addr: u64) -> bool {
        self.state_for(addr) == Some(LineState::Modified)
    }

    pub(super) fn invalidate(&mut self, addr: u64) {
        let (tag, set_index) = self.tag_and_set_index_for_addr(addr);

        for i in 0..self.config.num_ways {
            if self.sets[set_index][i].tag == tag {
                if self.sets[set_index][i].apply::<InvalidateLine>() {
                    self.sets[set_index][i].tag = 0;
                }
                break;
            }
        }
    }

    pub(super) fn add_pending_line_waiter(&mut self, request: T) {
        let (tag, set_index) = self.tag_and_set_index_for_addr(request.dst_addr());
        self.pending_line_waiters.push((tag, set_index, request));
    }

    pub(super) fn add_pending_nonposted_completion(&mut self, request: T) {
        let (tag, set_index) = self.tag_and_set_index_for_addr(request.dst_addr());
        self.pending_nonposted_completions
            .push((tag, set_index, request));
    }

    pub(super) fn add_pending_noallocate_completion(&mut self, request: T) {
        self.pending_noallocate_completions.push(request);
    }

    pub(super) fn add_pending_exclusive_write(&mut self, request: T) {
        if request.access_type() == AccessType::WriteNonPostedRequest {
            self.add_pending_nonposted_completion(request);
        } else {
            self.add_pending_line_waiter(request);
        }
    }

    pub(super) fn take_pending_line_waiters(&mut self, response: &T) -> Option<Vec<T>> {
        let (response_tag, response_set_index) =
            self.tag_and_set_index_for_addr(response.dst_addr());
        Self::take_pending_for_line(&mut self.pending_line_waiters, |tag, set_index| {
            tag == response_tag && set_index == response_set_index
        })
    }

    pub(super) fn take_pending_nonposted_completions(&mut self, response: &T) -> Option<Vec<T>> {
        let (response_tag, response_set_index) =
            self.tag_and_set_index_for_addr(response.dst_addr());
        Self::take_pending_for_line(&mut self.pending_nonposted_completions, |tag, set_index| {
            tag == response_tag && set_index == response_set_index
        })
    }

    pub(super) fn take_pending_noallocate_completion(&mut self, response: &T) -> Option<T> {
        let index = self
            .pending_noallocate_completions
            .iter()
            .position(|request| request.id() == response.id())?;
        Some(self.pending_noallocate_completions.remove(index))
    }

    fn take_pending_for_line<F>(
        pending: &mut Vec<(Tag, SetIndex, T)>,
        mut is_match: F,
    ) -> Option<Vec<T>>
    where
        F: FnMut(Tag, SetIndex) -> bool,
    {
        if pending
            .iter()
            .any(|(tag, set_index, _)| is_match(*tag, *set_index))
        {
            let all: Vec<(Tag, SetIndex, T)> = mem::take(pending);
            let (matching, not_matching) = all
                .into_iter()
                .partition(|(tag, set_index, _)| is_match(*tag, *set_index));
            *pending = not_matching;
            let matching = matching.into_iter().map(|(_, _, x)| x).collect();
            Some(matching)
        } else {
            None
        }
    }

    pub(super) fn take_pending_line_waiters_for_index_except_tag(
        &mut self,
        addr: u64,
        skip_tag: Tag,
    ) -> Option<Vec<T>> {
        let (_, set_index) = self.tag_and_set_index_for_addr(addr);
        Self::take_pending_for_line(&mut self.pending_line_waiters, |tag, pending_set_index| {
            pending_set_index == set_index && tag != skip_tag
        })
    }

    pub(super) fn device_id(&self) -> DeviceId {
        self.config.device_id
    }

    pub(super) fn has_pending_write_for_addr(&self, addr: u64) -> bool {
        let (tag, set_index) = self.tag_and_set_index_for_addr(addr);
        self.pending_line_waiters
            .iter()
            .chain(self.pending_nonposted_completions.iter())
            .any(|(pending_tag, pending_set_index, request)| {
                *pending_tag == tag
                    && *pending_set_index == set_index
                    && matches!(
                        request.access_type(),
                        AccessType::WriteRequest | AccessType::WriteNonPostedRequest
                    )
            })
    }

    pub(super) fn queue_blocked_device_request(&mut self, request: T) {
        self.blocked_device_requests.push_back(request);
    }

    pub(super) fn take_next_blocked_device_request(&mut self) -> Option<T> {
        self.blocked_device_requests.pop_front()
    }

    pub(super) fn has_active_barrier(&self) -> bool {
        self.active_barrier.is_some()
    }

    pub(super) fn set_active_barrier(&mut self, request: T) {
        self.active_barrier = Some(request);
        self.barrier_forwarded = false;
    }

    pub(super) fn mark_barrier_forwarded(&mut self) {
        self.barrier_forwarded = true;
    }

    pub(super) fn barrier_forwarded(&self) -> bool {
        self.barrier_forwarded
    }

    pub(super) fn active_barrier(&self) -> Option<T> {
        self.active_barrier.clone()
    }

    pub(super) fn clear_active_barrier(&mut self) -> Option<T> {
        self.barrier_forwarded = false;
        self.active_barrier.take()
    }

    pub(super) fn has_allocated_lines(&self) -> bool {
        self.sets
            .iter()
            .flat_map(|set| set.iter())
            .any(|entry| entry.line_state.is_allocated())
    }

    pub(super) fn has_outstanding_work(&self) -> bool {
        self.has_allocated_lines()
            || !self.pending_line_waiters.is_empty()
            || !self.pending_nonposted_completions.is_empty()
            || !self.pending_noallocate_completions.is_empty()
    }

    pub(super) fn is_coherent(&self) -> bool {
        self.coherency_manager_memory_map.is_some()
    }

    pub(super) fn coherency_manager_device_id_for_addr(
        &self,
        addr: u64,
    ) -> Result<DeviceId, SimError> {
        match &self.coherency_manager_memory_map {
            Some(memory_map) => memory_map
                .lookup(addr)
                .map(|(device_id, _)| device_id)
                .ok_or_else(|| {
                    SimError(format!(
                        "No coherency manager mapped for address 0x{addr:x}"
                    ))
                }),
            None => Err(SimError(
                "No coherency manager memory map configured".to_string(),
            )),
        }
    }

    pub(super) fn device_id_for_addr(&self, addr: u64) -> Result<DeviceId, SimError> {
        if self.is_coherent() {
            return self.coherency_manager_device_id_for_addr(addr);
        }

        self.config
            .memory_map
            .lookup(addr)
            .map(|(device_id, _)| device_id)
            .ok_or_else(|| SimError(format!("No backing memory mapped for address 0x{addr:x}")))
    }

    pub(super) fn bw_bytes_per_cycle(&self) -> usize {
        self.config.bw_bytes_per_cycle
    }
}

impl<T> ReadMemory for CacheContents<T>
where
    T: SimObject + CoherentAccess,
{
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

impl<T> ReadMemory for RefCell<CacheContents<T>>
where
    T: SimObject + CoherentAccess,
{
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

#[derive(Default, Clone)]
pub(super) struct CacheEntry {
    pub(super) line_state: LineState,
    pub(super) tag: Tag,
}

impl CacheEntry {
    pub(super) fn can_apply<TTransition: LineStateTransition>(&self) -> bool {
        TTransition::FROM.contains(&self.line_state)
    }

    pub(super) fn apply<TTransition: LineStateTransition>(&mut self) -> bool {
        if !self.can_apply::<TTransition>() {
            return false;
        }
        self.line_state = TTransition::TO;
        true
    }
}

pub(super) enum AllocateResult {
    Allocated,
    NeedsWriteback { evicted_modified_addr: u64 },
    Blocked,
}

#[cfg(test)]
impl AllocateResult {
    pub(super) fn is_allocated(&self) -> bool {
        matches!(self, Self::Allocated)
    }

    pub(super) fn is_blocked(&self) -> bool {
        matches!(self, Self::Blocked)
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use gwr_engine::test_helpers::start_test;
    use gwr_engine::types::AccessType;
    use gwr_track::id::Unique;

    use super::super::line_state::{AllocateExclusive, AllocateShared, GrantShared, LineState};
    use super::super::test_helpers::{test_access, test_config};
    use super::*;
    use crate::memory::memory_access::MemoryAccess;
    use crate::memory::memory_map::{DeviceId, MemoryMap};

    #[test]
    fn basic_ways() {
        let line_size_bytes = 32;
        let bw_bytes_per_cycle = 32;
        let num_sets = 1024;
        let num_ways = 4;
        let memory_map = Rc::new(MemoryMap::from_regions(&[(0, u64::MAX, DeviceId(0))]).unwrap());
        let config = CacheConfig::new(
            DeviceId(0),
            line_size_bytes,
            bw_bytes_per_cycle,
            num_sets,
            num_ways,
            8,
            &memory_map,
        );
        let mut state: CacheContents<MemoryAccess> = CacheContents::new(config);

        let mut addrs = Vec::new();
        let mut addr = 0x0100_0000;
        for _ in 0..num_ways + 1 {
            addrs.push(addr);
            addr += (line_size_bytes * num_sets * num_ways) as u64;
        }

        for addr in addrs.iter().take(num_ways) {
            assert_eq!(state.state_for(*addr), None);
            assert!(state.allocate::<AllocateShared>(*addr).is_allocated());
            assert_eq!(state.state_for(*addr), Some(LineState::AllocatedShared));
        }

        assert!(
            state
                .allocate::<AllocateShared>(addrs[num_ways])
                .is_blocked()
        );
        for addr in addrs.iter().take(num_ways) {
            assert_eq!(state.state_for(*addr), Some(LineState::AllocatedShared));
        }
    }

    #[test]
    fn invalidate() {
        let num_ways = 4;
        let config = test_config(1024, num_ways);
        let mut state: CacheContents<MemoryAccess> = CacheContents::new(config);

        let addr = 0x40000;
        assert!(state.allocate::<AllocateShared>(addr).is_allocated());
        assert_eq!(state.state_for(addr), Some(LineState::AllocatedShared));
        state.invalidate(addr);
        assert_eq!(state.state_for(addr), None);
    }

    #[test]
    fn cache_contents_tracks_pending_lines_by_line_and_set() {
        let engine = start_test(file!());
        let entity = engine.top().clone();
        let mut state: CacheContents<MemoryAccess> = CacheContents::new(test_config(2, 2));

        let line_a = 0x0000;
        let same_line = line_a + 8;
        let same_set_other_tag = 0x0040;
        let other_set = 0x0020;

        let read_a = test_access(&entity, AccessType::ReadRequest, line_a, DeviceId(1));
        let write_same_line =
            test_access(&entity, AccessType::WriteRequest, same_line, DeviceId(2));
        let read_same_set = test_access(
            &entity,
            AccessType::ReadRequest,
            same_set_other_tag,
            DeviceId(3),
        );
        let read_other_set = test_access(&entity, AccessType::ReadRequest, other_set, DeviceId(4));

        state.add_pending_line_waiter(read_a.clone());
        state.add_pending_line_waiter(write_same_line.clone());
        state.add_pending_line_waiter(read_same_set.clone());
        state.add_pending_line_waiter(read_other_set.clone());

        assert!(state.has_pending_write_for_addr(line_a));
        assert!(!state.has_pending_write_for_addr(other_set));

        let matching = state
            .take_pending_line_waiters(&test_access(
                &entity,
                AccessType::ReadResponse,
                line_a,
                DeviceId(9),
            ))
            .unwrap();
        assert_eq!(matching.len(), 2);
        assert!(matching.iter().any(|access| access.id() == read_a.id()));
        assert!(
            matching
                .iter()
                .any(|access| access.id() == write_same_line.id())
        );

        let (skip_tag, _) = state.tag_and_set_index_for_addr(line_a);
        let same_set = state
            .take_pending_line_waiters_for_index_except_tag(line_a, skip_tag)
            .unwrap();
        assert_eq!(same_set.len(), 1);
        assert_eq!(same_set[0].id(), read_same_set.id());

        let remaining = state.take_pending_line_waiters(&read_other_set).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id(), read_other_set.id());
        assert!(state.take_pending_line_waiters(&read_other_set).is_none());
        assert!(
            state
                .take_pending_line_waiters_for_index_except_tag(line_a, skip_tag)
                .is_none()
        );
    }

    #[test]
    fn cache_contents_barrier_and_blocked_queues_are_fifo() {
        let engine = start_test(file!());
        let entity = engine.top().clone();
        let mut state: CacheContents<MemoryAccess> = CacheContents::new(test_config(4, 2));
        let first = test_access(&entity, AccessType::ReadRequest, 0x100, DeviceId(1));
        let second = test_access(&entity, AccessType::ReadRequest, 0x120, DeviceId(2));
        let barrier = test_access(&entity, AccessType::BarrierRequest, 0x100, DeviceId(3));

        assert!(!state.has_active_barrier());
        assert!(!state.barrier_forwarded());
        state.set_active_barrier(barrier.clone());
        assert!(state.has_active_barrier());
        assert_eq!(state.active_barrier().unwrap().id(), barrier.id());
        state.mark_barrier_forwarded();
        assert!(state.barrier_forwarded());

        state.queue_blocked_device_request(first.clone());
        state.queue_blocked_device_request(second.clone());
        assert_eq!(
            state.take_next_blocked_device_request().unwrap().id(),
            first.id()
        );
        assert_eq!(
            state.take_next_blocked_device_request().unwrap().id(),
            second.id()
        );
        assert!(state.take_next_blocked_device_request().is_none());

        assert_eq!(state.clear_active_barrier().unwrap().id(), barrier.id());
        assert!(!state.has_active_barrier());
        assert!(!state.barrier_forwarded());
        assert!(state.clear_active_barrier().is_none());
    }

    #[test]
    fn cache_contents_reports_outstanding_work_from_allocations_and_pending_line_waiters() {
        let engine = start_test(file!());
        let entity = engine.top().clone();
        let mut state: CacheContents<MemoryAccess> = CacheContents::new(test_config(4, 2));

        assert!(!state.has_allocated_lines());
        assert!(!state.has_outstanding_work());

        let addr = 0x200;
        assert!(state.allocate::<AllocateShared>(addr).is_allocated());
        assert!(state.has_allocated_lines());
        assert!(state.has_outstanding_work());

        state.transition::<GrantShared>(addr);
        assert!(!state.has_allocated_lines());
        assert!(!state.has_outstanding_work());

        state.add_pending_line_waiter(test_access(
            &entity,
            AccessType::ReadRequest,
            addr,
            DeviceId(1),
        ));
        assert!(state.has_outstanding_work());
    }

    #[test]
    fn cache_contents_maps_coherency_managers_and_reports_missing_regions() {
        let state: CacheContents<MemoryAccess> = CacheContents::new(test_config(4, 2));
        assert!(!state.is_coherent());
        assert_eq!(state.device_id(), DeviceId(0));
        let err = state
            .coherency_manager_device_id_for_addr(0x100)
            .unwrap_err();
        assert!(err.0.contains("No coherency manager memory map configured"));

        let memory_map = Rc::new(MemoryMap::from_regions(&[(0x100, 0x20, DeviceId(7))]).unwrap());
        let coherent = CacheContents::<MemoryAccess>::new(
            test_config(4, 2).with_coherency_manager_memory_map(&memory_map),
        );
        assert!(coherent.is_coherent());
        assert_eq!(
            coherent
                .coherency_manager_device_id_for_addr(0x100)
                .unwrap(),
            DeviceId(7)
        );
        let err = coherent
            .coherency_manager_device_id_for_addr(0x200)
            .unwrap_err();
        assert!(
            err.0
                .contains("No coherency manager mapped for address 0x200")
        );
    }

    #[test]
    fn cache_contents_updates_existing_entries_and_reuses_evictable_ways() {
        let mut state: CacheContents<MemoryAccess> = CacheContents::new(test_config(1, 2));
        let first = 0x0000;
        let second = 0x0020;
        let third = 0x0040;

        assert!(state.allocate::<AllocateShared>(first).is_allocated());
        assert_eq!(
            state.find_way(state.tag_and_set_index_for_addr(first).0, 0),
            Some(0)
        );
        assert!(state.allocate::<AllocateExclusive>(second).is_allocated());
        assert!(state.allocate::<AllocateShared>(third).is_blocked());

        assert!(state.transition::<GrantShared>(first));
        assert_eq!(state.state_for(first), Some(LineState::Shared));
        state.grant_exclusive(second, false);
        assert!(state.allocate::<AllocateShared>(third).is_allocated());
        assert_eq!(state.state_for(third), Some(LineState::AllocatedShared));
        assert!(state.state_for(first).is_some() || state.state_for(second).is_some());
    }

    #[test]
    fn modified_victim_must_be_explicitly_evicted_before_reallocation() {
        let mut state: CacheContents<MemoryAccess> = CacheContents::new(test_config(1, 1));
        let first = 0x0000;
        let second = 0x0020;

        assert!(state.allocate::<AllocateExclusive>(first).is_allocated());
        assert!(state.grant_exclusive(first, true));
        assert_eq!(state.state_for(first), Some(LineState::Modified));

        let allocate_result = state.allocate::<AllocateShared>(second);
        let AllocateResult::NeedsWriteback {
            evicted_modified_addr,
        } = allocate_result
        else {
            panic!("modified victim should require writeback before reallocation");
        };
        assert_eq!(evicted_modified_addr, first);
        assert_eq!(state.state_for(first), Some(LineState::Modified));
        assert_eq!(state.state_for(second), None);

        assert!(state.evict_modified(evicted_modified_addr));
        assert_eq!(state.state_for(first), None);

        assert!(state.allocate::<AllocateShared>(second).is_allocated());
        assert_eq!(state.state_for(second), Some(LineState::AllocatedShared));
    }
}
