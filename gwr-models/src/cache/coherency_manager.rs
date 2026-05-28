// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A minimal directory-based coherence manager.
//!
//! The manager is a single fabric endpoint. It receives coherence traffic from
//! caches, resolves the backing memory from the access address, forwards memory
//! traffic back onto the same fabric, and receives the eventual memory
//! responses on the same `rx` port because those forwarded requests use the
//! manager device as their source.
//!
//! This implementation models an invalidate-based protocol:
//! - read misses request shared access from the directory
//! - write misses and shared write hits request exclusive access
//! - caches holding an old copy receive invalidates and respond with ack
//! - backing memory remains the source of data for reads
//! - posted writes may remain dirty in the owning cache until invalidation

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::{port_rx, take_option};
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Runnable, SimObject};
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;
use gwr_track::{debug, info, trace};

use crate::cache::traits::CoherentAccess;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::{DeviceId, MemoryMap};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoherenceState {
    Invalid,
    Shared,
    Exclusive,
    Modified,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoherenceOp {
    SharedRead,
    ExclusiveWrite,
    Invalidate,
    InvalidateAck,
    GrantShared,
    GrantExclusive,
}

#[derive(Clone)]
pub struct CoherencyManagerConfig {
    line_size_bytes: usize,
    device_id: DeviceId,
    backing_memory_map: MemoryMap,
}

impl CoherencyManagerConfig {
    #[must_use]
    pub fn new(line_size_bytes: usize, device_id: DeviceId, backing_memory_map: MemoryMap) -> Self {
        Self {
            line_size_bytes,
            device_id,
            backing_memory_map,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct CoherenceOpMetrics {
    shared_read: usize,
    exclusive_write: usize,
    invalidate: usize,
    invalidate_ack: usize,
    grant_shared: usize,
    grant_exclusive: usize,
}

impl CoherenceOpMetrics {
    pub(crate) fn total(&self) -> usize {
        self.shared_read
            + self.exclusive_write
            + self.invalidate
            + self.invalidate_ack
            + self.grant_shared
            + self.grant_exclusive
    }

    pub(crate) fn record(&mut self, op: CoherenceOp) {
        match op {
            CoherenceOp::SharedRead => self.shared_read += 1,
            CoherenceOp::ExclusiveWrite => self.exclusive_write += 1,
            CoherenceOp::Invalidate => self.invalidate += 1,
            CoherenceOp::InvalidateAck => self.invalidate_ack += 1,
            CoherenceOp::GrantShared => self.grant_shared += 1,
            CoherenceOp::GrantExclusive => self.grant_exclusive += 1,
        }
    }

    pub(crate) fn op_count(&self, op: CoherenceOp) -> usize {
        match op {
            CoherenceOp::SharedRead => self.shared_read,
            CoherenceOp::ExclusiveWrite => self.exclusive_write,
            CoherenceOp::Invalidate => self.invalidate,
            CoherenceOp::InvalidateAck => self.invalidate_ack,
            CoherenceOp::GrantShared => self.grant_shared,
            CoherenceOp::GrantExclusive => self.grant_exclusive,
        }
    }

    pub(crate) fn dump_stats(&self, entity: &Rc<Entity>, prefix: &str) {
        info!(entity ;
            "  Coherence {prefix}: total={}, shared_read={}, exclusive_write={}, invalidate={}, invalidate_ack={}, grant_shared={}, grant_exclusive={}",
            self.total(),
            self.shared_read,
            self.exclusive_write,
            self.invalidate,
            self.invalidate_ack,
            self.grant_shared,
            self.grant_exclusive
        );
    }
}

#[derive(Clone, Default)]
struct CoherencyManagerStats {
    coherence_ops_received: CoherenceOpMetrics,
    coherence_ops_sent: CoherenceOpMetrics,
}

#[derive(Clone)]
enum WaitingFor<T>
where
    T: SimObject + CoherentAccess,
{
    Invalidations {
        request: T,
        waiting_for: BTreeSet<DeviceId>,
    },
    ReadResponse {
        request: T,
    },
}

#[derive(Clone)]
struct DirectoryEntry<T>
where
    T: SimObject + CoherentAccess,
{
    sharers: BTreeSet<DeviceId>,
    owner: Option<DeviceId>,
    waiting_for: Option<WaitingFor<T>>,
    queued_requests: VecDeque<T>,
}

impl<T> Default for DirectoryEntry<T>
where
    T: SimObject + CoherentAccess,
{
    fn default() -> Self {
        Self {
            sharers: BTreeSet::new(),
            owner: None,
            waiting_for: None,
            queued_requests: VecDeque::new(),
        }
    }
}

struct DirectoryState<T>
where
    T: SimObject + CoherentAccess,
{
    config: CoherencyManagerConfig,
    entries: HashMap<u64, DirectoryEntry<T>>,
    blocked_requests: VecDeque<T>,
    active_barrier: Option<T>,
    stats: Rc<RefCell<CoherencyManagerStats>>,
}

impl<T> DirectoryState<T>
where
    T: SimObject + CoherentAccess,
{
    fn new(config: CoherencyManagerConfig, stats: Rc<RefCell<CoherencyManagerStats>>) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            blocked_requests: VecDeque::new(),
            active_barrier: None,
            stats,
        }
    }

    fn line_addr(&self, addr: u64) -> u64 {
        let line = self.config.line_size_bytes as u64;
        addr / line * line
    }

    fn resolve_backing_memory_device(
        &self,
        addr: u64,
        access_size_bytes: usize,
    ) -> Result<DeviceId, SimError> {
        let begin = addr;
        let end = begin + access_size_bytes as u64 - 1;
        let begin_device = self
            .config
            .backing_memory_map
            .lookup(begin)
            .ok_or_else(|| SimError(format!("No backing memory for address 0x{begin:x}")))?;
        let end_device = self
            .config
            .backing_memory_map
            .lookup(end)
            .ok_or_else(|| SimError(format!("No backing memory for address 0x{end:x}")))?;
        if begin_device.0 != end_device.0 {
            return sim_error!(
                "Coherence access [0x{begin:x},0x{end:x}] spans multiple backing memories"
            );
        }
        Ok(begin_device.0)
    }

    fn is_memory_response(&self, access: &T) -> bool {
        if !matches!(
            access.access_type(),
            AccessType::ReadResponse | AccessType::WriteNonPostedResponse
        ) {
            return false;
        }

        if access.dst_device() != self.config.device_id {
            return false;
        }

        self.resolve_backing_memory_device(access.dst_addr(), access.access_size_bytes())
            .is_ok_and(|backing_memory_device| access.src_device() == backing_memory_device)
    }

    fn has_outstanding_work(&self) -> bool {
        self.entries
            .values()
            .any(|entry| entry.waiting_for.is_some() || !entry.queued_requests.is_empty())
    }

    fn has_active_barrier(&self) -> bool {
        self.active_barrier.is_some()
    }

    fn set_active_barrier(&mut self, access: T) {
        self.active_barrier = Some(access);
    }

    fn active_barrier(&self) -> Option<T> {
        self.active_barrier.clone()
    }

    fn clear_active_barrier(&mut self) {
        self.active_barrier = None;
    }
}

fn log_directory_entry<T>(
    entity: &Rc<Entity>,
    line_addr: u64,
    prefix: &str,
    entry: &DirectoryEntry<T>,
) where
    T: SimObject + CoherentAccess,
{
    debug!(
        entity ;
        "{} line 0x{:x}: owner={:?} sharers={:?} waiting_for={} queued={}",
        prefix,
        line_addr,
        entry.owner,
        entry.sharers,
        entry.waiting_for.is_some(),
        entry.queued_requests.len()
    );
}

fn record_received_coherence_op<T>(state: &Rc<RefCell<DirectoryState<T>>>, op: Option<CoherenceOp>)
where
    T: SimObject + CoherentAccess,
{
    if let Some(op) = op {
        state
            .borrow()
            .stats
            .borrow_mut()
            .coherence_ops_received
            .record(op);
    }
}

fn record_sent_coherence_op<T>(state: &Rc<RefCell<DirectoryState<T>>>, op: Option<CoherenceOp>)
where
    T: SimObject + CoherentAccess,
{
    if let Some(op) = op {
        state
            .borrow()
            .stats
            .borrow_mut()
            .coherence_ops_sent
            .record(op);
    }
}

async fn send<T>(
    state: &Rc<RefCell<DirectoryState<T>>>,
    access: T,
    tx: &mut OutPort<T>,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    record_sent_coherence_op(state, access.coherence_op());
    tx.put(access)?.await;
    Ok(())
}

#[derive(EntityGet, EntityDisplay)]
pub struct CoherencyManager<T = MemoryAccess>
where
    T: SimObject + CoherentAccess,
{
    entity: Rc<Entity>,
    stats: Rc<RefCell<CoherencyManagerStats>>,
    state: Rc<RefCell<DirectoryState<T>>>,
    rx: RefCell<Option<InPort<T>>>,
    tx: RefCell<Option<OutPort<T>>>,
}

impl<T> CoherencyManager<T>
where
    T: SimObject + CoherentAccess,
{
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: CoherencyManagerConfig,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let stats = Rc::new(RefCell::new(CoherencyManagerStats::default()));
        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);
        let tx = OutPort::new(&entity, "tx");

        let rc_self = Rc::new(Self {
            entity,
            stats: stats.clone(),
            state: Rc::new(RefCell::new(DirectoryState::new(config, stats))),
            rx: RefCell::new(Some(rx)),
            tx: RefCell::new(Some(tx)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: CoherencyManagerConfig,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, config)
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        self.tx.borrow_mut().as_mut().unwrap().connect(port_state)
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        port_rx!(self.rx, state)
    }

    pub fn dump_stats(&self, _time_now_ns: f64) {
        let stats = self.stats.borrow();
        info!(self.entity ; "CoherencyManager {}:", self.entity.full_name());
        stats
            .coherence_ops_received
            .dump_stats(&self.entity, "received");
        stats.coherence_ops_sent.dump_stats(&self.entity, "sent");
    }

    pub fn op_received_count(&self, coherency_op: CoherenceOp) -> usize {
        self.stats
            .borrow()
            .coherence_ops_received
            .op_count(coherency_op)
    }

    pub fn total_received_count(&self) -> usize {
        self.stats.borrow().coherence_ops_received.total()
    }

    pub fn op_sent_count(&self, coherency_op: CoherenceOp) -> usize {
        self.stats
            .borrow()
            .coherence_ops_sent
            .op_count(coherency_op)
    }

    pub fn total_sent_count(&self) -> usize {
        self.stats.borrow().coherence_ops_sent.total()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for CoherencyManager<T>
where
    T: SimObject + CoherentAccess,
{
    async fn run(&self) -> SimResult {
        let state = self.state.clone();
        let entity = self.entity.clone();
        let mut rx = take_option!(self.rx);
        let mut tx = take_option!(self.tx);

        loop {
            let access = rx.get()?.await;
            record_received_coherence_op(&state, access.coherence_op());

            let maybe_line = if state.borrow().is_memory_response(&access) {
                trace!(entity ; "Coherence memory response {}", access);
                handle_mem_response(&entity, &state, &mut tx, access).await?
            } else {
                trace!(entity ; "Coherence request {}", access);
                handle_rx_access(&entity, &state, &mut tx, access).await?
            };

            progress_ready_work(&entity, &state, &mut tx, maybe_line).await?;
        }
    }
}

async fn handle_rx_access<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    access: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    if access.access_type() == AccessType::BarrierRequest {
        return handle_barrier_request(entity, state, tx, access).await;
    }

    if (access.access_type() != AccessType::Control
        || access.coherence_op() != Some(CoherenceOp::InvalidateAck))
        && state.borrow().has_active_barrier()
    {
        debug!(entity ; "Queue coherence access {} {} behind barrier", access.id(), access.access_type());
        state.borrow_mut().blocked_requests.push_back(access);
        return Ok(None);
    }

    let line_addr = {
        let state_ref = state.borrow();
        state_ref.resolve_backing_memory_device(access.dst_addr(), access.access_size_bytes())?;
        state_ref.line_addr(access.dst_addr())
    };

    if access.access_type() == AccessType::Control
        && access.coherence_op() == Some(CoherenceOp::InvalidateAck)
    {
        return handle_invalidate_ack(entity, state, tx, line_addr, access).await;
    }

    let should_queue = {
        let mut state_ref = state.borrow_mut();
        let entry = state_ref.entries.entry(line_addr).or_default();
        let is_owner_writeback = matches!(
            (
                access.access_type(),
                access.coherence_op(),
                &entry.waiting_for
            ),
            (
                AccessType::WriteRequest | AccessType::WriteNonPostedRequest,
                None,
                Some(WaitingFor::Invalidations { .. })
            )
        ) && entry.owner == Some(access.src_device());
        if entry.waiting_for.is_some() {
            log_directory_entry(
                entity,
                line_addr,
                "Queue request behind pending work;",
                entry,
            );
        }
        entry.waiting_for.is_some() && !is_owner_writeback
    };
    if should_queue {
        debug!(
            entity ;
            "Queue coherence access {} for line 0x{:x}: {} {:?}",
            access.id(),
            line_addr,
            access.access_type(),
            access.coherence_op()
        );
        state
            .borrow_mut()
            .entries
            .entry(line_addr)
            .or_default()
            .queued_requests
            .push_back(access);
        return Ok(None);
    }

    handle_cache_request(entity, state, tx, line_addr, access).await
}

async fn handle_barrier_request<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    access: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    if state.borrow().has_active_barrier() {
        debug!(entity ; "Queue barrier access {} behind active barrier", access.id());
        state.borrow_mut().blocked_requests.push_back(access);
        return Ok(None);
    }

    debug!(entity ; "Start barrier access {}", access.id());
    state.borrow_mut().set_active_barrier(access);
    let _ = try_complete_barrier(entity, state, tx).await?;
    Ok(None)
}

async fn try_complete_barrier<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
) -> Result<bool, SimError>
where
    T: SimObject + CoherentAccess,
{
    let barrier = {
        let state_ref = state.borrow();
        if !state_ref.has_active_barrier() || state_ref.has_outstanding_work() {
            return Ok(false);
        }
        state_ref.active_barrier()
    };
    let Some(barrier) = barrier else {
        return Ok(false);
    };

    debug!(entity ; "Complete barrier access {}", barrier.id());
    let response = barrier.to_response(&())?;
    tx.put(response)?.await;
    state.borrow_mut().clear_active_barrier();
    Ok(true)
}

async fn progress_ready_work<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    initial_line: Option<u64>,
) -> SimResult
where
    T: SimObject + CoherentAccess,
{
    let mut ready_lines = VecDeque::new();
    if let Some(line_addr) = initial_line {
        ready_lines.push_back(line_addr);
    }

    loop {
        let mut progressed = false;

        while let Some(line_addr) = ready_lines.pop_front() {
            let next = {
                let mut state_ref = state.borrow_mut();
                let entry = state_ref.entries.entry(line_addr).or_default();
                if entry.waiting_for.is_some() {
                    log_directory_entry(entity, line_addr, "Line still busy;", entry);
                    None
                } else {
                    if !entry.queued_requests.is_empty() {
                        log_directory_entry(
                            entity,
                            line_addr,
                            "Dequeue next waiting request;",
                            entry,
                        );
                    }
                    entry.queued_requests.pop_front()
                }
            };

            if let Some(next) = next {
                if let Some(next_line) =
                    handle_cache_request(entity, state, tx, line_addr, next).await?
                {
                    ready_lines.push_back(next_line);
                }
                try_complete_barrier(entity, state, tx).await?;
                progressed = true;
            }
        }

        loop {
            if state.borrow().has_active_barrier() {
                break;
            }

            let request = { state.borrow_mut().blocked_requests.pop_front() };
            let Some(request) = request else {
                break;
            };
            progressed = true;
            debug!(entity ; "Replay blocked coherence access {} {}", request.id(), request.access_type());
            if let Some(line_addr) = handle_rx_access(entity, state, tx, request).await? {
                ready_lines.push_back(line_addr);
            }
        }

        let barrier_completed = try_complete_barrier(entity, state, tx).await?;
        if barrier_completed {
            progressed = true;
        }

        if !progressed {
            break;
        }
    }

    Ok(())
}

async fn handle_cache_request<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    line_addr: u64,
    access: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    debug!(
        entity ;
        "Handle cache request {} for line 0x{:x}: {} {:?} from {}",
        access.id(),
        line_addr,
        access.access_type(),
        access.coherence_op(),
        access.src_device()
    );
    match (access.access_type(), access.coherence_op()) {
        (AccessType::ReadRequest, Some(CoherenceOp::SharedRead))
        | (AccessType::ReadRequest, None) => {
            start_shared_read(entity, state, tx, line_addr, access).await
        }
        (AccessType::WriteRequest, Some(CoherenceOp::ExclusiveWrite))
        | (AccessType::WriteNonPostedRequest, Some(CoherenceOp::ExclusiveWrite)) => {
            start_exclusive_write(entity, state, tx, line_addr, access).await
        }
        (AccessType::WriteRequest, None) | (AccessType::WriteNonPostedRequest, None) => {
            forward_owner_write(entity, state, tx, line_addr, access).await
        }
        _ => sim_error!(
            "{}: unsupported request with AccessType {} and coherence op {:?}",
            entity,
            access.access_type(),
            access.coherence_op()
        ),
    }
}

async fn start_shared_read<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    line_addr: u64,
    access: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    let requester = access.src_device();
    let invalidate_targets = {
        let mut state_ref = state.borrow_mut();
        let entry = state_ref.entries.entry(line_addr).or_default();
        let mut targets = BTreeSet::new();
        if let Some(owner) = entry.owner
            && owner != requester
        {
            targets.insert(owner);
        }
        if targets.is_empty() {
            entry.waiting_for = Some(WaitingFor::ReadResponse {
                request: access.clone(),
            });
        } else {
            entry.waiting_for = Some(WaitingFor::Invalidations {
                request: access.clone(),
                waiting_for: targets.clone(),
            });
        }
        log_directory_entry(entity, line_addr, "Start shared read;", entry);
        targets
    };

    if invalidate_targets.is_empty() {
        debug!(
            entity ;
            "Shared read access {} for line 0x{:x} can read from memory immediately",
            access.id(),
            line_addr
        );
        issue_read_to_memory(entity, state, tx, access).await
    } else {
        for target in invalidate_targets {
            let invalidate = access
                .clone()
                .with_access_type(AccessType::Control)
                .with_routing(target, state.borrow().config.device_id)
                .with_coherence_op(Some(CoherenceOp::Invalidate));
            debug!(
                entity ;
                "Shared read access {} for line 0x{:x} invalidates holder {} with {}",
                access.id(),
                line_addr,
                target,
                invalidate
            );
            send(state, invalidate, tx).await?;
        }
        Ok(None)
    }
}

async fn start_exclusive_write<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    line_addr: u64,
    access: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    let requester = access.src_device();
    let invalidate_targets = {
        let mut state_ref = state.borrow_mut();
        let entry = state_ref.entries.entry(line_addr).or_default();
        let mut targets = entry.sharers.clone();
        targets.remove(&requester);
        if let Some(owner) = entry.owner
            && owner != requester
        {
            targets.insert(owner);
        }
        if !targets.is_empty() {
            entry.waiting_for = Some(WaitingFor::Invalidations {
                request: access.clone(),
                waiting_for: targets.clone(),
            });
        }
        log_directory_entry(entity, line_addr, "Start exclusive write;", entry);
        targets
    };

    if invalidate_targets.is_empty() {
        debug!(
            entity ;
            "Exclusive write access {} for line 0x{:x} can proceed immediately",
            access.id(),
            line_addr
        );
        grant_exclusive_without_writeback(entity, state, tx, line_addr, access).await
    } else {
        for target in invalidate_targets {
            let invalidate = access
                .clone()
                .with_access_type(AccessType::Control)
                .with_routing(target, state.borrow().config.device_id)
                .with_coherence_op(Some(CoherenceOp::Invalidate));
            debug!(
                entity ;
                "Exclusive write access {} for line 0x{:x} invalidates holder {} with {}",
                access.id(),
                line_addr,
                target,
                invalidate
            );
            send(state, invalidate, tx).await?;
        }
        Ok(None)
    }
}

async fn forward_owner_write<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    line_addr: u64,
    access: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    let requester = access.src_device();
    let is_owner = state
        .borrow()
        .entries
        .get(&line_addr)
        .is_some_and(|entry| entry.owner == Some(requester));
    if !is_owner {
        debug!(
            entity ;
            "Write access {} for line 0x{:x} from {} is not owner; upgrade to exclusive request",
            access.id(),
            line_addr,
            requester
        );
        let upgraded = access.with_coherence_op(Some(CoherenceOp::ExclusiveWrite));
        return start_exclusive_write(entity, state, tx, line_addr, upgraded).await;
    }

    let dst_addr = access.dst_addr();
    let access_size_bytes = access.access_size_bytes();
    let forwarded = access
        .clone()
        .with_routing(
            state
                .borrow()
                .resolve_backing_memory_device(dst_addr, access_size_bytes)?,
            state.borrow().config.device_id,
        )
        .with_coherence_op(None);
    debug!(
        entity ;
        "Owner write access {} for line 0x{:x} forwards to memory as {}",
        access.id(),
        line_addr,
        forwarded
    );
    tx.put(forwarded)?.await;
    Ok(None)
}

async fn handle_invalidate_ack<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    line_addr: u64,
    ack: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    debug!(
        entity ;
        "Invalidate ack access {} for line 0x{:x} from {}",
        ack.id(),
        line_addr,
        ack.src_device()
    );
    let next = {
        let mut state_ref = state.borrow_mut();
        let entry = state_ref.entries.entry(line_addr).or_default();
        let Some(WaitingFor::Invalidations {
            request,
            waiting_for,
        }) = entry.waiting_for.as_mut()
        else {
            return sim_error!("Unexpected invalidate ack for line 0x{line_addr:x}");
        };
        waiting_for.remove(&ack.src_device());
        if waiting_for.is_empty() {
            let request = request.clone();
            entry.waiting_for = None;
            entry.owner = None;
            entry.sharers.remove(&ack.src_device());
            Some(request)
        } else {
            entry.sharers.remove(&ack.src_device());
            if entry.owner == Some(ack.src_device()) {
                entry.owner = None;
            }
            log_directory_entry(
                entity,
                line_addr,
                "Still waiting after invalidate ack;",
                entry,
            );
            None
        }
    };

    if let Some(request) = next {
        debug!(
            entity ;
            "All invalidations complete for line 0x{:x}; resume access {} {} {:?}",
            line_addr,
            request.id(),
            request.access_type(),
            request.coherence_op()
        );
        match request.coherence_op() {
            Some(CoherenceOp::SharedRead) | None
                if request.access_type() == AccessType::ReadRequest =>
            {
                state
                    .borrow_mut()
                    .entries
                    .entry(line_addr)
                    .or_default()
                    .waiting_for = Some(WaitingFor::ReadResponse {
                    request: request.clone(),
                });
                issue_read_to_memory(entity, state, tx, request).await
            }
            Some(CoherenceOp::ExclusiveWrite) => {
                grant_exclusive_without_writeback(entity, state, tx, line_addr, request).await
            }
            _ => sim_error!("Unsupported pending request after invalidations"),
        }
    } else {
        Ok(None)
    }
}

async fn issue_read_to_memory<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    access: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    let dst_addr = access.dst_addr();
    let access_size_bytes = access.access_size_bytes();
    let forwarded = access
        .clone()
        .with_routing(
            state
                .borrow()
                .resolve_backing_memory_device(dst_addr, access_size_bytes)?,
            state.borrow().config.device_id,
        )
        .with_coherence_op(None);
    debug!(entity ; "Issue memory read for access {} as {}", access.id(), forwarded);
    tx.put(forwarded)?.await;
    Ok(None)
}

async fn grant_exclusive_without_writeback<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    line_addr: u64,
    access: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    {
        let mut state_ref = state.borrow_mut();
        let entry = state_ref.entries.entry(line_addr).or_default();
        entry.sharers.clear();
        entry.owner = Some(access.src_device());
        log_directory_entry(entity, line_addr, "Grant exclusive ownership;", entry);
    }

    let src_device = access.src_device();
    let grant = access
        .with_access_type(AccessType::Control)
        .with_routing(src_device, state.borrow().config.device_id)
        .with_coherence_op(Some(CoherenceOp::GrantExclusive));
    debug!(
        entity ;
        "Grant exclusive ownership for access {} on line 0x{:x} without memory write as {}",
        grant.id(),
        line_addr,
        grant
    );
    send(state, grant, tx).await?;
    Ok(Some(line_addr))
}

async fn send_grant<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    request: T,
    response: T,
    line_addr: u64,
    coherence_op: CoherenceOp,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    let grant = response
        .with_routing(request.src_device(), state.borrow().config.device_id)
        .with_coherence_op(Some(coherence_op));
    debug!(entity ; "Return grant for request {} on line 0x{:x} as {}", request.id(), line_addr, grant);
    send(state, grant, tx).await?;
    Ok(Some(line_addr))
}

async fn handle_mem_response<T>(
    entity: &Rc<Entity>,
    state: &Rc<RefCell<DirectoryState<T>>>,
    tx: &mut OutPort<T>,
    response: T,
) -> Result<Option<u64>, SimError>
where
    T: SimObject + CoherentAccess,
{
    let line_addr = {
        let state_ref = state.borrow();
        state_ref.line_addr(response.dst_addr())
    };
    debug!(
        entity ;
        "Handle memory response {} for line 0x{:x}: {} {:?}",
        response.id(),
        line_addr,
        response.access_type(),
        response.coherence_op()
    );
    let waiting_for = state
        .borrow()
        .entries
        .get(&line_addr)
        .and_then(|entry| entry.waiting_for.clone());

    match waiting_for {
        Some(WaitingFor::ReadResponse { request }) => {
            {
                let mut state_ref = state.borrow_mut();
                let entry = state_ref.entries.entry(line_addr).or_default();
                entry.waiting_for = None;
                entry.owner = None;
                entry.sharers.insert(request.src_device());
                log_directory_entry(entity, line_addr, "Complete shared read;", entry);
            }
            send_grant(
                entity,
                state,
                tx,
                request,
                response,
                line_addr,
                CoherenceOp::GrantShared,
            )
            .await
        }
        _ => sim_error!("Unexpected memory response for line 0x{line_addr:x}"),
    }
}
