// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Logical round-robin striped memory.
//!
//! [`MemoryStriper`] exposes one logical memory range backed by a
//! [`MemoryMap`]. Consecutive stripes are assigned round-robin to backing
//! regions in increasing address order. [`MemoryStriperConfig`] requires a
//! positive stripe size and provides independent limits on parent requests and
//! downstream fragments through [`MemoryStriperConfig::with_admission_limits`].
//!
//! # Address mapping
//!
//! For two banks and 64-byte stripes, logical offsets 0, 64, 128 and 192 map to
//! bank A offset 0, bank B offset 0, bank A offset 64 and bank B offset 64.
//! Offsets are relative to the logical device's base; each backing region's
//! base is added when forming its downstream address. Logical and backing
//! addresses belong to separate maps and may overlap. Backing regions may
//! themselves name other stripers.
//!
//! # Requests and completion
//!
//! Reads, posted writes and nonposted writes are split at stripe boundaries.
//! Reads and nonposted writes return one response for the original request
//! after all fragments complete; posted writes produce no response. Downstream
//! responses may arrive out of order. Control and barrier packets are
//! unsupported, and the model supplies no ordering guarantees of its own.
//!
//! Packets implement [`SimObject`] and [`StripingAccess`]. [`StripingAccess`]
//! documents fragment IDs, metadata preservation, response validation and
//! traffic-only completion. Read payloads are not assembled from fragment
//! responses. The model adds no configurable processing or bandwidth delay;
//! connect fabric, memory or rate-limiter models to represent those costs.
//!
//! # Construction
//!
//! This example uses two 512-byte banks and 64-byte stripes to back a 1024-byte
//! logical device.
//!
//! ```no_run
//! use gwr_engine::engine::Engine;
//! use gwr_engine::types::DeviceId;
//! use gwr_models::memory::memory_access::MemoryAccess;
//! use gwr_models::memory::memory_map::MemoryMap;
//! use gwr_models::memory::striper::{MemoryStriper, MemoryStriperConfig};
//!
//! fn main() -> Result<(), gwr_engine::types::SimError> {
//!     let mut engine = Engine::default();
//!     let clock = engine.default_clock();
//!     let mut backing = MemoryMap::new();
//!     backing.insert(0x10_0000, 512, DeviceId(2))?;
//!     backing.insert(0x20_0000, 512, DeviceId(3))?;
//!     let config = MemoryStriperConfig::new(0, 1024, 64, DeviceId(1), backing)
//!         .with_admission_limits(32, 32);
//!     let _striper = MemoryStriper::<MemoryAccess>::new_and_register(
//!         &engine,
//!         &clock,
//!         engine.top(),
//!         "striped_memory",
//!         config,
//!     )?;
//!     Ok(())
//! }
//! ```

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::ops::RangeInclusive;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::{connect_tx, port_rx, take_option};
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, Runnable, SimObject};
use gwr_engine::types::{AccessType, DeviceId, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::entity::Entity;
use gwr_track::trace;
use gwr_track::tracker::aka::Aka;

use super::checked_last_address;
use super::memory_map::{MemoryMap, MemoryRegion};
use super::traits::{ReadMemory, StripingAccess};

#[derive(Clone)]
pub struct MemoryStriperConfig {
    base_address: u64,
    capacity_bytes: usize,
    device_id: DeviceId,
    backing_memory_map: MemoryMap,
    stripe_size_bytes: usize,
    max_inflight_parents: usize,
    max_inflight_fragments: usize,
}

impl MemoryStriperConfig {
    #[must_use]
    pub fn new(
        base_address: u64,
        capacity_bytes: usize,
        stripe_size_bytes: usize,
        device_id: DeviceId,
        backing_memory_map: MemoryMap,
    ) -> Self {
        Self {
            base_address,
            capacity_bytes,
            device_id,
            backing_memory_map,
            stripe_size_bytes,
            max_inflight_parents: 32,
            max_inflight_fragments: 32,
        }
    }

    /// Set independent limits on admitted original requests and downstream
    /// fragments awaiting responses. Both default to 32.
    ///
    /// Read and nonposted-write parents retain admission until their response
    /// is accepted upstream, including while that response is blocked. Posted
    /// writes retain admission until their final fragment is accepted
    /// downstream. Only fragments awaiting responses occupy slots, which are
    /// released when a valid response arrives.
    ///
    /// A full fragment table pauses generation for all request types, including
    /// posted writes. Large requests stream through available slots, and one
    /// parent can use the entire fragment limit.
    #[must_use]
    pub fn with_admission_limits(
        mut self,
        max_inflight_parents: usize,
        max_inflight_fragments: usize,
    ) -> Self {
        self.max_inflight_parents = max_inflight_parents;
        self.max_inflight_fragments = max_inflight_fragments;
        self
    }

    pub fn address_range(&self) -> Result<RangeInclusive<u64>, SimError> {
        let capacity = u64::try_from(self.capacity_bytes).map_err(|e| SimError(e.to_string()))?;
        let last = checked_last_address(self.base_address, capacity).ok_or_else(|| {
            SimError(format!(
                "invalid memory striper logical address range: base_address={:#x}, capacity_bytes={}",
                self.base_address, self.capacity_bytes
            ))
        })?;
        Ok(self.base_address..=last)
    }

    pub fn validate(&self) -> SimResult {
        self.address_range()?;
        if self.stripe_size_bytes == 0
            || self.max_inflight_parents == 0
            || self.max_inflight_fragments == 0
        {
            return sim_error!(
                "memory striper stripe_size_bytes={}, max_inflight_parents={} and max_inflight_fragments={} must be positive",
                self.stripe_size_bytes,
                self.max_inflight_parents,
                self.max_inflight_fragments
            );
        }
        let banks = self.backing_memory_map.num_regions();
        if banks == 0 {
            return sim_error!(
                "memory striper backing memory map is empty (device_id={})",
                self.device_id
            );
        }
        // Use a wider domain for capacities: a region may cover all u64
        // addresses.
        let stripe = self.stripe_size_bytes as u128;
        let bytes = self.capacity_bytes as u128;
        let full = bytes / stripe;
        let tail = bytes % stripe;
        for (bank, region) in self.backing_memory_map.regions().enumerate() {
            let count = full / banks as u128 + u128::from((bank as u128) < full % banks as u128);
            let required = count * stripe
                + if bank as u128 == full % banks as u128 {
                    tail
                } else {
                    0
                };
            let available = u128::from(region.end) - u128::from(region.start) + 1;
            if required > available {
                return sim_error!(
                    "memory striper bank {bank} (device {}) needs {required} bytes, has {available}",
                    region.device
                );
            }
            if region.device == self.device_id {
                return sim_error!(
                    "memory striper backing map points to its own device {}",
                    self.device_id
                );
            }
        }
        Ok(())
    }
}

struct Parent<T> {
    request: T,
    issued_bytes: usize,
    outstanding_children: usize,
}

struct Child<T> {
    parent_id: u64,
    request: T,
}

struct State<T> {
    parents: BTreeMap<u64, Parent<T>>,
    children: BTreeMap<u64, Child<T>>,
    ready_parent_ids: VecDeque<u64>,
    next_parent_id: u64,
    next_child_id: u64,
}

impl<T> Default for State<T> {
    fn default() -> Self {
        Self {
            parents: BTreeMap::new(),
            children: BTreeMap::new(),
            ready_parent_ids: VecDeque::new(),
            next_parent_id: 0,
            next_child_id: 0,
        }
    }
}

struct WorkerState<T> {
    entity: Rc<Entity>,
    config: MemoryStriperConfig,
    banks: Vec<MemoryRegion>,
    transactions: RefCell<State<T>>,
    changed: Repeated<()>,
}

#[derive(EntityGet, EntityDisplay)]
pub struct MemoryStriper<T>
where
    T: SimObject + StripingAccess,
{
    entity: Rc<Entity>,
    spawner: Spawner,
    worker_state: Rc<WorkerState<T>>,
    dev_rx: RefCell<Option<InPort<T>>>,
    dev_tx: RefCell<Option<OutPort<T>>>,
    mem_rx: RefCell<Option<InPort<T>>>,
    mem_tx: RefCell<Option<OutPort<T>>>,
}

impl<T> MemoryStriper<T>
where
    T: SimObject + StripingAccess,
{
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: MemoryStriperConfig,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, config)
    }

    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: MemoryStriperConfig,
    ) -> Result<Rc<Self>, SimError> {
        config
            .validate()
            .map_err(|e| SimError(format!("{parent}::{name}: {e}")))?;
        let entity = Rc::new(Entity::new(parent, name));
        let dev_rx = InPort::new_with_renames(engine, clock, &entity, "dev_rx", aka);
        let mem_rx = InPort::new_with_renames(engine, clock, &entity, "mem_rx", aka);
        let dev_tx = OutPort::new_with_renames(&entity, "dev_tx", aka);
        let mem_tx = OutPort::new_with_renames(&entity, "mem_tx", aka);
        let worker_state = Rc::new(WorkerState {
            entity: entity.clone(),
            banks: config.backing_memory_map.regions().cloned().collect(),
            config,
            transactions: RefCell::new(State::default()),
            changed: Repeated::default(),
        });
        let model = Rc::new(Self {
            entity,
            spawner: engine.spawner(),
            worker_state,
            dev_rx: RefCell::new(Some(dev_rx)),
            dev_tx: RefCell::new(Some(dev_tx)),
            mem_rx: RefCell::new(Some(mem_rx)),
            mem_tx: RefCell::new(Some(mem_tx)),
        });
        engine.register(model.clone());
        Ok(model)
    }

    pub fn port_dev_rx(&self) -> PortStateResult<T> {
        port_rx!(self.dev_rx, state)
    }
    pub fn port_mem_rx(&self) -> PortStateResult<T> {
        port_rx!(self.mem_rx, state)
    }
    pub fn connect_port_dev_tx(&self, port: PortStateResult<T>) -> SimResult {
        connect_tx!(self.dev_tx, connect ; port)
    }
    pub fn connect_port_mem_tx(&self, port: PortStateResult<T>) -> SimResult {
        connect_tx!(self.mem_tx, connect ; port)
    }
    #[must_use]
    pub fn base_address(&self) -> u64 {
        self.worker_state.config.base_address
    }
    #[must_use]
    pub fn capacity_bytes(&self) -> usize {
        self.worker_state.config.capacity_bytes
    }
}

impl<T> WorkerState<T>
where
    T: SimObject + StripingAccess,
{
    fn striper_error(&self, message: impl std::fmt::Display) -> SimError {
        SimError(format!("{}: {message}", self.entity))
    }

    fn validate_request(&self, request: &T) -> SimResult {
        if !matches!(
            request.access_type(),
            AccessType::ReadRequest | AccessType::WriteRequest | AccessType::WriteNonPostedRequest
        ) {
            return sim_error!(
                "{}: unsupported memory striper request access_type: received {}, expected ReadRequest, WriteRequest, or WriteNonPostedRequest (dst_addr={:#x}, access_size_bytes={}, dst_device={})",
                self.entity,
                request.access_type(),
                request.dst_addr(),
                request.access_size_bytes(),
                request.dst_device()
            );
        }
        let range = self.config.address_range()?;
        let end = u64::try_from(request.access_size_bytes())
            .ok()
            .and_then(|bytes| checked_last_address(request.dst_addr(), bytes));
        if request.dst_device() != self.config.device_id
            || !range.contains(&request.dst_addr())
            || end.is_none_or(|last| !range.contains(&last))
        {
            return sim_error!(
                "{}: invalid or out of bounds memory striper request: dst_addr={:#x}, access_size_bytes={}, dst_device={}; expected dst_device={} and logical range {:#x}..={:#x}",
                self.entity,
                request.dst_addr(),
                request.access_size_bytes(),
                request.dst_device(),
                self.config.device_id,
                range.start(),
                range.end()
            );
        }
        Ok(())
    }

    async fn receive_requests(&self, mut rx: InPort<T>) -> SimResult {
        loop {
            while self.transactions.borrow().parents.len() >= self.config.max_inflight_parents {
                self.changed.listen().await;
            }
            let request = rx.get()?.await;
            self.validate_request(&request)?;
            {
                let mut state = self.transactions.borrow_mut();
                let parent_id = state.next_parent_id;
                state.next_parent_id = parent_id
                    .checked_add(1)
                    .ok_or_else(|| self.striper_error("striper parent identities exhausted"))?;
                state.parents.insert(
                    parent_id,
                    Parent {
                        request,
                        issued_bytes: 0,
                        outstanding_children: 0,
                    },
                );
                trace!(self.entity ; "Admitted parent_id={parent_id}: {}", state.parents[&parent_id].request);
            }
            self.changed.notify();
        }
    }

    /// Generate just one fragment; neither a huge parent nor blocked output can
    /// grow the child table or an outgoing fragment queue without bound.
    fn next_fragment(&self) -> Result<Option<(u64, T)>, SimError> {
        let mut state = self.transactions.borrow_mut();
        if state.children.len() >= self.config.max_inflight_fragments {
            return Ok(None);
        }
        let Some((&parent_id, parent)) = state
            .parents
            .iter()
            .find(|(_, parent)| parent.issued_bytes < parent.request.access_size_bytes())
        else {
            return Ok(None);
        };
        let offset =
            parent.request.dst_addr() - self.config.base_address + parent.issued_bytes as u64;
        let stripe_size = self.config.stripe_size_bytes as u64;
        let stripe = offset / stripe_size;
        let within = offset % stripe_size;
        let bank = &self.banks[(stripe % self.banks.len() as u64) as usize];
        let local = stripe / self.banks.len() as u64 * stripe_size + within;
        let bytes = (self.config.stripe_size_bytes - within as usize)
            .min(parent.request.access_size_bytes() - parent.issued_bytes);
        let child_id = state.next_child_id;
        let request = parent
            .request
            .stripe_fragment(
                bank.start + local,
                bytes,
                bank.device,
                self.config.device_id,
                child_id,
            )
            .map_err(|error| self.striper_error(error))?;
        state.next_child_id = child_id
            .checked_add(1)
            .ok_or_else(|| self.striper_error("striper child identities exhausted"))?;
        let parent = state.parents.get_mut(&parent_id).unwrap();
        parent.issued_bytes += bytes;
        if request.access_type() != AccessType::WriteRequest {
            parent.outstanding_children += 1;
            state.children.insert(
                child_id,
                Child {
                    parent_id,
                    request: request.clone(),
                },
            );
        }
        trace!(self.entity ; "Created fragment parent_id={parent_id} fragment_id={child_id} bank_device={} translated_address={:#x} bytes={bytes}: {}", bank.device, bank.start + local, request);
        Ok(Some((parent_id, request)))
    }

    async fn send_fragments(&self, mut tx: OutPort<T>) -> SimResult {
        loop {
            match self.next_fragment()? {
                Some((parent_id, request)) => {
                    let posted = request.access_type() == AccessType::WriteRequest;
                    let fragment_id = request.src_addr();
                    tx.put(request)?.await;
                    trace!(self.entity ; "Issued fragment parent_id={parent_id} fragment_id={fragment_id}");
                    if posted {
                        let mut state = self.transactions.borrow_mut();
                        let finished = state.parents.get(&parent_id).is_some_and(|parent| {
                            parent.issued_bytes == parent.request.access_size_bytes()
                        });
                        if finished {
                            state.parents.remove(&parent_id);
                            trace!(self.entity ; "Completed posted parent_id={parent_id}");
                        }
                        drop(state);
                        self.changed.notify();
                    }
                }
                None => self.changed.listen().await,
            }
        }
    }

    fn validate_response(&self, expected: &T, response: &T) -> SimResult {
        let child_id = response.src_addr();
        let response_type = if expected.access_type() == AccessType::ReadRequest {
            AccessType::ReadResponse
        } else {
            AccessType::WriteNonPostedResponse
        };
        if response.access_type() != response_type {
            return Err(self.response_error(child_id, "stripe response type mismatch"));
        }
        if response.dst_device() != expected.src_device()
            || response.src_device() != expected.dst_device()
        {
            return Err(self.response_error(child_id, "stripe response routing mismatch"));
        }
        if response.dst_addr() != expected.dst_addr()
            || response.access_size_bytes() != expected.access_size_bytes()
        {
            return Err(self.response_error(child_id, "stripe response range mismatch"));
        }
        expected
            .validate_stripe_response(response)
            .map_err(|error| self.response_error(child_id, error))
    }

    fn response_error(&self, child_id: u64, message: impl std::fmt::Display) -> SimError {
        self.striper_error(format_args!(
            "malformed memory stripe response for child_id={child_id}: {message}"
        ))
    }

    fn accept_response(&self, response: &T) -> SimResult {
        let mut state = self.transactions.borrow_mut();
        let child_id = response.src_addr();
        let Some(child) = state.children.get(&child_id) else {
            return sim_error!(
                "{}: unexpected or duplicate memory stripe response for child_id={child_id} (src_device={}, dst_device={})",
                self.entity,
                response.src_device(),
                response.dst_device()
            );
        };
        self.validate_response(&child.request, response)?;
        let parent_id = child.parent_id;
        state.children.remove(&child_id);
        let parent = state.parents.get_mut(&parent_id).unwrap();
        parent.outstanding_children -= 1;
        if parent.outstanding_children == 0
            && parent.issued_bytes == parent.request.access_size_bytes()
        {
            state.ready_parent_ids.push_back(parent_id);
        }
        trace!(self.entity ; "Accepted response parent_id={parent_id} fragment_id={child_id}: {}", response);
        Ok(())
    }

    async fn receive_responses(&self, mut rx: InPort<T>) -> SimResult {
        loop {
            let response = rx.get()?.await;
            trace!(self.entity ; "Memory response fragment_id={}: {}", response.src_addr(), response);
            self.accept_response(&response)?;
            self.changed.notify();
        }
    }

    async fn return_parents(&self, mut tx: OutPort<T>) -> SimResult {
        loop {
            let next = self.transactions.borrow_mut().ready_parent_ids.pop_front();
            match next {
                Some(parent_id) => {
                    let response = {
                        let state = self.transactions.borrow();
                        let request = &state.parents[&parent_id].request;
                        if request.access_type() == AccessType::ReadRequest {
                            request.to_response(&EmptyMemory)
                        } else {
                            request.to_ack()
                        }
                        .map_err(|error| self.striper_error(error))?
                    };
                    tx.put(response)?.await;
                    self.transactions.borrow_mut().parents.remove(&parent_id);
                    trace!(self.entity ; "Completed parent_id={parent_id}");
                    self.changed.notify();
                }
                None => self.changed.listen().await,
            }
        }
    }
}

struct EmptyMemory;
impl ReadMemory for EmptyMemory {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for MemoryStriper<T>
where
    T: SimObject + StripingAccess,
{
    async fn run(&self) -> SimResult {
        let dev_rx = take_option!(self.dev_rx);
        let mem_tx = take_option!(self.mem_tx);
        let mem_rx = take_option!(self.mem_rx);
        let dev_tx = take_option!(self.dev_tx);
        let request_state = self.worker_state.clone();
        self.spawner
            .spawn(async move { request_state.receive_requests(dev_rx).await });
        let fragment_state = self.worker_state.clone();
        self.spawner
            .spawn(async move { fragment_state.send_fragments(mem_tx).await });
        let return_state = self.worker_state.clone();
        self.spawner
            .spawn(async move { return_state.return_parents(dev_tx).await });
        self.worker_state.receive_responses(mem_rx).await
    }
}

#[cfg(test)]
mod tests;
