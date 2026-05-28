// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Directory ownership is serialized per coherence line. Output is sent
//! independently so a blocked grant cannot stop incoming invalidation acks.
//!
//! # Concepts and terminology
//!
//! - **Coherence line:** an aligned range of `line_size_bytes` bytes, used as
//!   the unit of ownership and serialization. Each data request must fit in one
//!   line and one backing-memory region.
//! - **Directory entry:** the manager's record for a line: its sharers,
//!   optional owner, active operation, and queued requests. Different lines can
//!   progress independently; requests for the same line wait behind its active
//!   operation.
//! - **Sharer:** a cache registered as holding a shared, readable copy. An
//!   allocating read registers its requester when backing memory responds; a
//!   nonallocating read does not add a sharer.
//! - **Owner:** the cache granted exclusive permission to modify a line. The
//!   directory tracks that permission, while the cache tracks whether its copy
//!   is dirty. Other readers or writers must invalidate the owner before using
//!   backing memory or acquiring ownership.
//! - **Acquisition and grant:** `SharedRead` requests readable data, while
//!   `ExclusiveWrite` requests permission to write. `GrantShared` returns data
//!   from backing memory; `GrantExclusive` gives the requester ownership after
//!   conflicting copies have been invalidated. A nonallocating write instead
//!   forwards its payload to backing memory without retaining ownership. Posted
//!   writes use nonposted backing-memory transactions so the directory can wait
//!   for completion; their acknowledgements are consumed by the manager.
//! - **Invalidation and acknowledgement:** `Invalidate` revokes a cached copy;
//!   `InvalidateAck` confirms completion. Each invalidation round has a
//!   directory-owned generation token. An acknowledgement must match that
//!   token, the line range, and an expected sender before it can advance the
//!   operation.
//! - **Writeback:** an owner's full-line dirty data sent to backing memory
//!   through the manager as a nonposted write. It releases directory ownership,
//!   but remains outstanding until memory acknowledges it. During invalidation,
//!   the manager waits for both memory completion and the cache's invalidation
//!   acknowledgement before issuing the waiting request.
//! - **Child identity:** the cache-owned fill token carried by a request and
//!   its backing-memory response. The manager checks this identity, the
//!   response range, type, and sender before completing the active memory
//!   operation. It is separate from the directory's invalidation generation
//!   token.
//! - **Barrier:** a drain of earlier manager operations. Later requests wait,
//!   while responses, writebacks, and invalidation acknowledgements needed to
//!   finish earlier work remain serviceable. A barrier does not flush resident
//!   dirty cache lines or establish a global memory consistency model.
//! - **Output queue:** protocol and backing-memory packets are emitted in FIFO
//!   order by a separate worker. A blocked output port therefore does not stop
//!   the manager receiving packets needed to complete an active operation.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::{port_rx, take_option};
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, Runnable, SimObject};
use gwr_engine::types::{AccessType, DeviceId, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;

use crate::cache::traits::{CoherenceToken, CoherentAccess};
use crate::memory::CacheHintType;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::memory::traits::CacheFillToken;

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
    Writeback,
}

#[derive(Clone)]
pub struct CoherencyManagerConfig {
    pub(crate) line_size_bytes: usize,
    pub(crate) device_id: DeviceId,
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

    pub fn validate(&self) -> SimResult {
        if self.line_size_bytes == 0 {
            return Err(coherence_error(
                "coherency manager line size must be positive",
            ));
        }
        for region in self.backing_memory_map.regions() {
            if !(region.start as u128).is_multiple_of(self.line_size_bytes as u128)
                || !(region.end as u128 + 1).is_multiple_of(self.line_size_bytes as u128)
            {
                return Err(coherence_error(
                    "coherency manager memory ranges must align to coherence lines",
                ));
            }
        }
        Ok(())
    }
}

#[derive(EntityGet, EntityDisplay)]
pub struct CoherencyManager<T = MemoryAccess>
where
    T: SimObject + CoherentAccess,
{
    entity: Rc<Entity>,
    spawner: Spawner,
    state: RefCell<Directory<T>>,
    rx: RefCell<Option<InPort<T>>>,
    tx: RefCell<Option<OutPort<T>>>,
}

impl<T: SimObject + CoherentAccess> CoherencyManager<T> {
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: CoherencyManagerConfig,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, config)
    }

    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: CoherencyManagerConfig,
    ) -> Result<Rc<Self>, SimError> {
        config.validate()?;
        let entity = Rc::new(Entity::new(parent, name));
        let value = Rc::new(Self {
            spawner: engine.spawner(),
            state: RefCell::new(Directory::new(config)),
            rx: RefCell::new(Some(InPort::new_with_renames(
                engine, clock, &entity, "rx", aka,
            ))),
            tx: RefCell::new(Some(OutPort::new_with_renames(&entity, "tx", aka))),
            entity,
        });
        engine.register(value.clone());
        Ok(value)
    }

    pub fn connect_port_tx(&self, state: PortStateResult<T>) -> SimResult {
        self.tx
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| coherence_error("manager tx already taken"))?
            .connect(state)
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        port_rx!(self.rx, state)
    }

    pub fn received_coherence_op_count(&self, op: CoherenceOp) -> usize {
        self.state.borrow().received[op as usize]
    }

    pub fn sent_coherence_op_count(&self, op: CoherenceOp) -> usize {
        self.state.borrow().sent[op as usize]
    }

    pub fn dump_stats(&self, _time_now_ns: f64) {
        gwr_track::info!(self.entity ; "Coherence received: {}, sent: {}", self.received_coherence_message_count(), self.sent_coherence_message_count());
    }

    pub fn received_coherence_message_count(&self) -> usize {
        self.state.borrow().received.iter().sum()
    }

    pub fn sent_coherence_message_count(&self) -> usize {
        self.state.borrow().sent.iter().sum()
    }
}

#[async_trait(?Send)]
impl<T: SimObject + CoherentAccess> Runnable for CoherencyManager<T> {
    async fn run(&self) -> SimResult {
        let mut rx = take_option!(self.rx);
        let mut tx = take_option!(self.tx);
        let output = Rc::new(RefCell::new(VecDeque::new()));
        let changed = Repeated::default();
        let pending = output.clone();
        let wake = changed.clone();
        self.spawner.spawn(async move {
            loop {
                let next = pending.borrow_mut().pop_front();
                if let Some(packet) = next {
                    tx.put(packet)?.await;
                } else {
                    wake.listen().await;
                }
            }
        });
        loop {
            let packet = rx.get()?.await;
            let messages = self.state.borrow_mut().handle_packet(packet)?;
            output.borrow_mut().extend(messages);
            changed.notify();
        }
    }
}

struct Entry<T> {
    sharer_device_ids: BTreeSet<DeviceId>,
    owner_device_id: Option<DeviceId>,
    active: Option<Active<T>>,
    queued: VecDeque<T>,
}

impl<T> Default for Entry<T> {
    fn default() -> Self {
        Self {
            sharer_device_ids: BTreeSet::new(),
            owner_device_id: None,
            active: None,
            queued: VecDeque::new(),
        }
    }
}

struct Active<T> {
    request: T,
    phase: Phase<T>,
}

enum Phase<T> {
    Invalidations {
        pending_sender_device_ids: BTreeSet<DeviceId>,
        token: CoherenceToken,
        writeback: Option<T>,
    },
    Memory(T),
}

struct Directory<T> {
    config: CoherencyManagerConfig,
    entries: BTreeMap<u64, Entry<T>>,
    /// Lines waiting for invalidation acknowledgements or memory responses.
    active_line_count: usize,
    invalidation_token_owner: Rc<()>,
    next_invalidation_ordinal: u64,
    barrier: Option<T>,
    blocked: VecDeque<T>,
    received: [usize; 7],
    sent: [usize; 7],
}

impl<T: SimObject + CoherentAccess> Directory<T> {
    fn new(config: CoherencyManagerConfig) -> Self {
        Self {
            config,
            entries: BTreeMap::new(),
            active_line_count: 0,
            invalidation_token_owner: Rc::new(()),
            next_invalidation_ordinal: 0,
            barrier: None,
            blocked: VecDeque::new(),
            received: [0; 7],
            sent: [0; 7],
        }
    }

    fn handle_packet(&mut self, packet: T) -> Result<Vec<T>, SimError> {
        if packet.dst_device() != self.config.device_id {
            return Err(coherence_error(
                "coherence packet addressed to another device",
            ));
        }
        let mut output = Vec::new();
        let line = self.line_start_address(packet.dst_addr());
        let op = packet.coherence_op();
        if packet.coherence_op() == Some(CoherenceOp::InvalidateAck) {
            self.handle_invalidation_ack(&packet, &mut output)?;
        } else if matches!(
            packet.access_type(),
            AccessType::ReadResponse | AccessType::WriteNonPostedResponse
        ) {
            self.handle_memory_response(packet, &mut output)?;
        } else if packet.coherence_op() == Some(CoherenceOp::Writeback) {
            self.handle_owner_writeback(packet, &mut output)?;
        } else if self.barrier.is_some() {
            self.blocked.push_back(packet);
        } else if packet.access_type() == AccessType::BarrierRequest {
            self.barrier = Some(packet);
        } else {
            self.accept_request(packet, &mut output)?;
        }
        if let Some(op) = op {
            self.received[op as usize] += 1;
        }
        self.advance_queued_requests(line, &mut output)?;
        self.advance_barriers(&mut output)?;
        for message in &output {
            if let Some(op) = message.coherence_op() {
                self.sent[op as usize] += 1;
            }
        }
        Ok(output)
    }

    fn handle_invalidation_ack(&mut self, packet: &T, output: &mut Vec<T>) -> SimResult {
        let line = self.line_start_address(packet.dst_addr());
        let entry = self
            .entries
            .get_mut(&line)
            .ok_or_else(|| coherence_error("unexpected invalidation ack"))?;
        let Some(Active {
            phase:
                Phase::Invalidations {
                    pending_sender_device_ids,
                    token,
                    ..
                },
            ..
        }) = &mut entry.active
        else {
            return Err(coherence_error("invalidation ack has no active operation"));
        };
        if packet.access_type() != AccessType::Control
            || packet.dst_addr() != line
            || packet.access_size_bytes() != self.config.line_size_bytes
            || !packet
                .coherence_token()
                .is_some_and(|value| token.matches(value))
            || !pending_sender_device_ids.contains(&packet.src_device())
        {
            return Err(coherence_error(
                "stale/foreign invalidation acknowledgement",
            ));
        }
        pending_sender_device_ids.remove(&packet.src_device());
        entry.sharer_device_ids.remove(&packet.src_device());
        if entry.owner_device_id == Some(packet.src_device()) {
            entry.owner_device_id = None;
        }
        self.complete_invalidations(line, output)
    }

    fn complete_invalidations(&mut self, line: u64, output: &mut Vec<T>) -> SimResult {
        let entry = self.entries.get_mut(&line).unwrap();
        if matches!(
            &entry.active,
            Some(Active {
                phase: Phase::Invalidations {
                    pending_sender_device_ids,
                    writeback: None,
                    ..
                },
                ..
            }) if pending_sender_device_ids.is_empty()
        ) {
            let active = entry.active.take().unwrap();
            self.active_line_count -= 1;
            self.issue_memory_request_or_grant(line, active.request, output)?;
        }
        Ok(())
    }

    fn handle_memory_response(&mut self, packet: T, output: &mut Vec<T>) -> SimResult {
        let line = self.line_start_address(packet.dst_addr());
        let entry = self
            .entries
            .get_mut(&line)
            .ok_or_else(|| coherence_error("unexpected memory response"))?;
        let Some(active) = &mut entry.active else {
            return Err(coherence_error("memory response has no active request"));
        };
        let expected = match &active.phase {
            Phase::Memory(expected)
            | Phase::Invalidations {
                writeback: Some(expected),
                ..
            } => expected,
            Phase::Invalidations { .. } => {
                return Err(coherence_error("memory response has no active request"));
            }
        };
        if packet.src_device() != expected.dst_device()
            || packet.dst_addr() != expected.dst_addr()
            || packet.access_size_bytes() != expected.access_size_bytes()
            || packet.access_type()
                != if expected.access_type() == AccessType::ReadRequest {
                    AccessType::ReadResponse
                } else {
                    AccessType::WriteNonPostedResponse
                }
            || !fill_tokens_match(packet.cache_fill_token(), expected.cache_fill_token())
        {
            return Err(coherence_error(
                "memory response does not match active child",
            ));
        }
        if let Phase::Invalidations { writeback, .. } = &mut active.phase {
            *writeback = None;
            return self.complete_invalidations(line, output);
        }
        let request = &active.request;
        let requester_device_id = request.src_device();
        let posted_write = request.access_type() == AccessType::WriteRequest;
        let read = request.access_type() == AccessType::ReadRequest;
        if read && request.cache_hint() == CacheHintType::Allocate {
            entry.sharer_device_ids.insert(requester_device_id);
        }
        entry.active = None;
        self.active_line_count -= 1;
        if posted_write {
            return Ok(());
        }
        output.push(
            packet
                .with_routing(requester_device_id, self.config.device_id)
                .with_coherence_op(if read {
                    Some(CoherenceOp::GrantShared)
                } else {
                    None
                }),
        );
        Ok(())
    }

    fn handle_owner_writeback(&mut self, packet: T, output: &mut Vec<T>) -> SimResult {
        let line = self.line_start_address(packet.dst_addr());
        let backing_memory_device_id = self.backing_memory_device_for_request(&packet)?;
        let entry = self
            .entries
            .get_mut(&line)
            .ok_or_else(|| coherence_error("writeback has no directory owner"))?;
        if packet.access_type() != AccessType::WriteRequest
            || packet.dst_addr() != line
            || packet.access_size_bytes() != self.config.line_size_bytes
            || entry.owner_device_id != Some(packet.src_device())
            || packet.cache_fill_token().is_none()
        {
            return Err(coherence_error("invalid owner writeback"));
        }
        let forwarded = packet
            .clone()
            .with_access_type(AccessType::WriteNonPostedRequest)
            .with_routing(backing_memory_device_id, self.config.device_id)
            .with_coherence_op(None)
            .with_coherence_token(None);
        match &mut entry.active {
            None => {
                entry.active = Some(Active {
                    request: packet,
                    phase: Phase::Memory(forwarded.clone()),
                });
                self.active_line_count += 1;
            }
            Some(Active {
                phase: Phase::Invalidations { writeback, .. },
                ..
            }) if writeback.is_none() => {
                *writeback = Some(forwarded.clone());
            }
            _ => return Err(coherence_error("writeback conflicts with active operation")),
        }
        entry.owner_device_id = None;
        output.push(forwarded);
        Ok(())
    }

    fn advance_queued_requests(&mut self, line: u64, output: &mut Vec<T>) -> SimResult {
        while let Some(entry) = self.entries.get_mut(&line) {
            if entry.active.is_some() {
                break;
            }
            let Some(next) = entry.queued.pop_front() else {
                break;
            };
            self.start_line_operation(line, next, output)?;
        }
        Ok(())
    }

    fn advance_barriers(&mut self, output: &mut Vec<T>) -> SimResult {
        loop {
            if self.barrier.is_some() && self.active_line_count == 0 {
                output.push(self.barrier.take().unwrap().to_ack()?);
            }
            if self.barrier.is_none()
                && let Some(next) = self.blocked.pop_front()
            {
                if next.access_type() == AccessType::BarrierRequest {
                    self.barrier = Some(next);
                } else {
                    let line = self.line_start_address(next.dst_addr());
                    self.accept_request(next, output)?;
                    self.advance_queued_requests(line, output)?;
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn accept_request(&mut self, packet: T, output: &mut Vec<T>) -> SimResult {
        self.backing_memory_device_for_request(&packet)?;
        if !matches!(
            (packet.access_type(), packet.coherence_op()),
            (
                AccessType::ReadRequest,
                Some(CoherenceOp::SharedRead) | None
            ) | (AccessType::Control, Some(CoherenceOp::ExclusiveWrite))
                | (
                    AccessType::WriteRequest | AccessType::WriteNonPostedRequest,
                    Some(CoherenceOp::ExclusiveWrite) | None
                )
        ) {
            return Err(coherence_error("unsupported coherence request"));
        }
        if packet.cache_fill_token().is_none() {
            return Err(coherence_error("coherence request lacks child identity"));
        }
        let line = self.line_start_address(packet.dst_addr());
        let entry = self.entries.entry(line).or_default();
        if entry.active.is_some() {
            entry.queued.push_back(packet);
            return Ok(());
        }
        self.start_line_operation(line, packet, output)
    }

    fn start_line_operation(&mut self, line: u64, packet: T, output: &mut Vec<T>) -> SimResult {
        let entry = self.entries.entry(line).or_default();
        let mut target_device_ids = BTreeSet::new();
        if packet.access_type() != AccessType::ReadRequest {
            target_device_ids.extend(entry.sharer_device_ids.iter().copied());
        }
        if let Some(owner_device_id) = entry.owner_device_id {
            target_device_ids.insert(owner_device_id);
        }
        target_device_ids.remove(&packet.src_device());
        if target_device_ids.is_empty() {
            return self.issue_memory_request_or_grant(line, packet, output);
        }
        let invalidation_ordinal = self.next_invalidation_ordinal;
        self.next_invalidation_ordinal = invalidation_ordinal
            .checked_add(1)
            .ok_or_else(|| coherence_error("directory invalidation identity exhausted"))?;
        let token = CoherenceToken {
            owner: self.invalidation_token_owner.clone(),
            ordinal: invalidation_ordinal,
        };
        let child_fill_token = packet.cache_fill_token().unwrap().clone();
        let control = packet
            .build_coherence_child_request(
                AccessType::Control,
                line,
                self.config.line_size_bytes,
                child_fill_token,
            )?
            .with_coherence_op(Some(CoherenceOp::Invalidate))
            .with_coherence_token(Some(token.clone()));
        for target_device_id in &target_device_ids {
            output.push(
                control
                    .clone()
                    .with_routing(*target_device_id, self.config.device_id),
            );
        }
        self.entries.get_mut(&line).unwrap().active = Some(Active {
            request: packet,
            phase: Phase::Invalidations {
                pending_sender_device_ids: target_device_ids,
                token,
                writeback: None,
            },
        });
        self.active_line_count += 1;
        Ok(())
    }

    fn issue_memory_request_or_grant(
        &mut self,
        line: u64,
        packet: T,
        output: &mut Vec<T>,
    ) -> SimResult {
        let allocate = packet.cache_hint() == CacheHintType::Allocate;
        if packet.access_type() == AccessType::ReadRequest || !allocate {
            if !allocate && packet.access_type() != AccessType::ReadRequest {
                self.entries
                    .get_mut(&line)
                    .unwrap()
                    .sharer_device_ids
                    .remove(&packet.src_device());
            }
            let mut forwarded = packet
                .clone()
                .with_routing(
                    self.backing_memory_device_for_request(&packet)?,
                    self.config.device_id,
                )
                .with_coherence_op(None)
                .with_coherence_token(None);
            // Track backing-memory completion even when the requester does
            // not expect a response to its posted write.
            if packet.access_type() == AccessType::WriteRequest {
                forwarded = forwarded.with_access_type(AccessType::WriteNonPostedRequest);
            }
            self.entries.get_mut(&line).unwrap().active = Some(Active {
                request: packet,
                phase: Phase::Memory(forwarded.clone()),
            });
            self.active_line_count += 1;
            output.push(forwarded);
        } else {
            let response = (if packet.access_type() == AccessType::Control {
                packet.clone()
            } else {
                packet
                    .clone()
                    .with_access_type(AccessType::WriteNonPostedRequest)
            })
            .to_ack()?
            .with_coherence_op(Some(CoherenceOp::GrantExclusive))
            .with_routing(packet.src_device(), self.config.device_id);
            let entry = self.entries.get_mut(&line).unwrap();
            entry.owner_device_id = Some(packet.src_device());
            entry.sharer_device_ids.clear();
            entry.active = None;
            output.push(response);
        }
        Ok(())
    }

    fn backing_memory_device_for_request(&self, packet: &T) -> Result<DeviceId, SimError> {
        let address = packet.dst_addr();
        let bytes = u64::try_from(packet.access_size_bytes())
            .map_err(|e| coherence_error(&e.to_string()))?;
        let last = address
            .checked_add(
                bytes
                    .checked_sub(1)
                    .ok_or_else(|| coherence_error("zero size coherence data request"))?,
            )
            .ok_or_else(|| coherence_error("coherence request range overflows"))?;
        if self.line_start_address(address) != self.line_start_address(last) {
            return Err(coherence_error("coherence request spans directory lines"));
        }
        let (backing_memory_device_id, offset) = self
            .config
            .backing_memory_map
            .lookup(address)
            .ok_or_else(|| coherence_error("no backing memory at request address"))?;
        if self.config.backing_memory_map.lookup(last)
            != Some((backing_memory_device_id, offset + (last - address)))
        {
            return Err(coherence_error(
                "coherence request crosses backing memory boundary",
            ));
        }
        Ok(backing_memory_device_id)
    }

    fn line_start_address(&self, address: u64) -> u64 {
        address - address % self.config.line_size_bytes as u64
    }
}

fn fill_tokens_match(a: Option<&CacheFillToken>, b: Option<&CacheFillToken>) -> bool {
    matches!((a,b), (Some(a),Some(b)) if Rc::ptr_eq(&a.owner, &b.owner) && a.ordinal == b.ordinal)
}

fn coherence_error(message: &str) -> SimError {
    SimError(message.into())
}

#[cfg(test)]
mod tests;
