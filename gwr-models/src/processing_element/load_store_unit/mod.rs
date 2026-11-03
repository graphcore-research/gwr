// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A model of a Load/Store Unit for a Processing Element (PE).
//!
//! The LSU supports a user-defined number of outstanding operations
//! and can be shared by multiple simultaneous tasks within the PE.
//! Each memory-map region must begin and end on an LSU access boundary;
//! construction rejects maps that cannot contain full aligned packets.

//! # Ports
//!
//! The LSU uses:
//!  - One [input port](gwr_engine::port::InPort): `rx`
//!  - One [output port](gwr_engine::port::OutPort): `tx`

use std::cell::{Cell, RefCell};
use std::cmp::min;
use std::collections::VecDeque;
use std::fmt;
use std::fmt::Formatter;
use std::ops::Range;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::{connect_tx, port_rx};
use gwr_engine::engine::Engine;
use gwr_engine::events::once::Once;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::time::clock::phase::END;
use gwr_engine::traits::{Event, Routable, Runnable};
use gwr_engine::types::{AccessType, DeviceId, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::debug;
use gwr_track::entity::{Entity, EntityGroup};
use gwr_track::tracker::aka::Aka;

use super::task::ProgramTaskConfig;
use crate::processing_element::tensors::TensorView;

mod request_arbiter;

use request_arbiter::RequestArbiter;

use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::processing_element::{ActivityLanes, ProcessingElementConfig};

#[derive(EntityGet, EntityDisplay)]
pub struct LoadStoreUnit {
    entity: Rc<Entity>,
    spawner: Spawner,
    clock: Clock,

    /// Bytes serviced by each ordinary aligned LSU access.
    max_access_size_bytes: usize,

    rx: RefCell<Option<InPort<MemoryAccess>>>,
    tx: RefCell<Option<OutPort<MemoryAccess>>>,

    state: Rc<LsuState>,
}

impl fmt::Debug for LoadStoreUnit {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadStoreUnit")
            .field("entity", &self.entity)
            .finish()
    }
}

impl LoadStoreUnit {
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        aka: Option<&Aka>,
        pe_config: &ProcessingElementConfig,
        memory_map: &Rc<MemoryMap>,
        device_id: DeviceId,
    ) -> Result<Rc<Self>, SimError> {
        let access_bytes = pe_config.lsu_access_bytes as u128;
        for region in memory_map.regions() {
            // Use the exclusive end in u128 so a region may include u64::MAX.
            if !u128::from(region.start).is_multiple_of(access_bytes)
                || !(u128::from(region.end) + 1).is_multiple_of(access_bytes)
            {
                return sim_error!(
                    "Memory map region {:#x}..={:#x} must be aligned to LSU access size {} bytes",
                    region.start,
                    region.end,
                    pe_config.lsu_access_bytes
                );
            }
        }

        let entity = Rc::new(Entity::new(parent, "lsu"));
        let tx = OutPort::new_with_renames(&entity, "tx", aka);
        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);

        let num_active_requests = pe_config.num_active_requests;
        let max_access_size_bytes = pe_config.lsu_access_bytes;
        let overhead_size_bytes = pe_config.overhead_size_bytes;
        let mut requests = Vec::with_capacity(num_active_requests);
        for _ in 0..num_active_requests {
            requests.push(ActiveRequestSlot::default());
        }

        let state = LsuState {
            entity: entity.clone(),
            memory_map: memory_map.clone(),
            device_id,
            overhead_size_bytes,
            active_request_slots: RefCell::new(requests),
            pending_request_indices: RefCell::new(VecDeque::new()),
            new_request: Repeated::new(()),
            slot_available: Repeated::new(()),
            request_arbiter: RequestArbiter::default(),
        };
        let spawner = engine.spawner();
        let rc_self = Rc::new(Self {
            entity,
            spawner,
            clock: clock.clone(),
            max_access_size_bytes,
            rx: RefCell::new(Some(rx)),
            tx: RefCell::new(Some(tx)),
            state: Rc::new(state),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<MemoryAccess>) -> SimResult {
        connect_tx!(self.tx, connect ; port_state)
    }

    pub fn port_rx(&self) -> PortStateResult<MemoryAccess> {
        port_rx!(self.rx, state)
    }

    pub(crate) fn validate_program_views(&self, config: &ProgramTaskConfig) -> SimResult {
        for view in config.inputs.iter().chain(&config.outputs) {
            self.validate_mapped_view(view)?;
        }
        Ok(())
    }

    /// Check every physical byte touched by a view is mapped. Untouched
    /// strided holes need not be mapped; packed elements count unique bytes.
    /// This is independent of request issuing and allocates no request slots.
    pub(crate) fn validate_mapped_view(&self, view: &TensorView) -> SimResult {
        let mapped = self
            .state
            .memory_map
            .logical_ranges()
            .try_fold(0usize, |sum, region| {
                sum.checked_add(view.num_access_bytes_in(region))
                    .ok_or_else(|| SimError("mapped view byte count overflow".into()))
            })?;
        if mapped != view.layout().num_access_bytes() {
            return sim_error!("tensor view contains unmapped touched bytes");
        }
        Ok(())
    }

    /// Transfer physical ranges using aligned, fixed-size ordinary requests.
    pub(crate) async fn access_ranges(
        &self,
        access_type: AccessType,
        ranges: impl IntoIterator<Item = Range<u128>>,
        activity_lanes: &Rc<RefCell<ActivityLanes>>,
        activity_name: &str,
        group: &Rc<EntityGroup>,
    ) -> SimResult {
        let completion = Rc::new(TransferCompletion::new());

        let mut activity_guard = None;

        let issue_result = async {
            for range in ranges {
                let mut access_address = u64::try_from(range.start)
                    .expect("tensor construction guarantees range starts fit in u64");
                let mut bytes_remaining = usize::try_from(range.end - range.start)
                    .expect("tensor construction guarantees range lengths fit in usize");

                while bytes_remaining > 0 {
                    if self.state.memory_map.lookup(access_address).is_none() {
                        return sim_error!("Unmapped LSU address {access_address:#x}");
                    }
                    // Service aligned full-size packets while advancing by
                    // the logical tensor bytes they cover. Packet construction
                    // validates that the entire access fits one mapped region.
                    let offset = (access_address % self.max_access_size_bytes as u64) as usize;
                    let packet_address = access_address - offset as u64;
                    let access_size_bytes = self.max_access_size_bytes;
                    let covered_bytes = min(self.max_access_size_bytes - offset, bytes_remaining);
                    let request_turn = self.state.request_arbiter.request().await;
                    let request_slot_idx = self.state.allocate_request_slot().await;
                    // Own only acquired credits until the worker takes over.
                    let mut preissue_slot = RequestSlotGuard {
                        state: self.state.clone(),
                        slot: request_slot_idx,
                        armed: true,
                    };
                    if activity_guard.is_none() {
                        // Reuse a lane released in this tick before allocating
                        // one.
                        self.clock.wait_phase(END).await;
                        activity_guard = Some(ActivityLanes::begin_in_group(
                            activity_lanes,
                            activity_name,
                            group,
                        ));
                    }

                    let access = self.state.create_memory_access(
                        access_type,
                        access_size_bytes,
                        packet_address,
                        request_slot_idx,
                    )?;

                    {
                        let state = self.state.clone();
                        let completion = completion.clone();
                        completion.request_started()?;
                        self.spawner.spawn(async move {
                            let response_ready_event =
                                state.make_request_to_port_driver(request_slot_idx, access)?;
                            // Actual queue/port worker now owns the slot. It
                            // will be released only
                            // by handling the actual reply.
                            preissue_slot.disarm();
                            drop(request_turn);
                            response_ready_event.listen().await;
                            let result = state.handle_response_in_slot(request_slot_idx);
                            completion.request_completed()?;
                            result
                        });
                    }

                    bytes_remaining -= covered_bytes;
                    if bytes_remaining != 0 {
                        access_address = access_address
                            .checked_add(covered_bytes as u64)
                            .expect("validated tensor access remains in the address space");
                    }
                }
            }

            Ok(())
        }
        .await;

        completion.finish_issuing()?;
        completion.complete.listen().await;

        issue_result
    }
}

#[async_trait(?Send)]
impl Runnable for LoadStoreUnit {
    async fn run(&self) -> SimResult {
        let rx = self.rx.borrow_mut().take().unwrap();
        let tx = self.tx.borrow_mut().take().unwrap();

        {
            let state = self.state.clone();
            self.spawner.spawn(async move { run_tx(state, tx).await });
        }

        run_rx(self.state.clone(), rx).await
    }
}

struct TransferCompletion {
    outstanding_requests: Cell<usize>,
    issuing: Cell<bool>,
    complete: Once<()>,
}

impl TransferCompletion {
    fn new() -> Self {
        Self {
            outstanding_requests: Cell::new(0),
            issuing: Cell::new(true),
            complete: Once::default(),
        }
    }

    fn request_started(&self) -> SimResult {
        let outstanding = self
            .outstanding_requests
            .get()
            .checked_add(1)
            .ok_or_else(|| SimError("LSU request count overflows".to_string()))?;
        self.outstanding_requests.set(outstanding);
        Ok(())
    }

    fn request_completed(&self) -> SimResult {
        let outstanding = self
            .outstanding_requests
            .get()
            .checked_sub(1)
            .ok_or_else(|| {
                SimError("LSU transfer completed more requests than it issued".to_string())
            })?;
        self.outstanding_requests.set(outstanding);
        if !self.issuing.get() && outstanding == 0 {
            self.complete.notify()?;
        }
        Ok(())
    }

    fn finish_issuing(&self) -> SimResult {
        self.issuing.set(false);
        if self.outstanding_requests.get() == 0 {
            self.complete.notify()?;
        }
        Ok(())
    }
}

async fn run_rx(state: Rc<LsuState>, mut rx: InPort<MemoryAccess>) -> SimResult {
    loop {
        let response = rx.get()?.await;
        state.put_response_in_active_request_slot(response)?;
    }
}

async fn run_tx(state: Rc<LsuState>, mut tx: OutPort<MemoryAccess>) -> SimResult {
    loop {
        if !state.try_handle_next_active_request(&mut tx).await? {
            // Nothing was handled. Wait until the queue is changed.
            state.new_request.listen().await;
        }
    }
}

// Exists only after allocate_request_slot successfully set in_use. Once the
// internal worker has enqueued the actual packet it disarms this pre-issue
// guard; request/reply code then owns the slot until real response handling.
struct RequestSlotGuard {
    state: Rc<LsuState>,
    slot: usize,
    armed: bool,
}

impl RequestSlotGuard {
    // Calling a method captures the whole guard in the async worker, rather
    // than capturing only its armed field and dropping the slot owner early.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RequestSlotGuard {
    fn drop(&mut self) {
        if self.armed {
            let mut slots = self.state.active_request_slots.borrow_mut();
            assert!(slots[self.slot].in_use);
            assert!(slots[self.slot].request.is_none());
            slots[self.slot].in_use = false;
            drop(slots);
            self.state.slot_available.notify();
        }
    }
}

struct LsuState {
    entity: Rc<Entity>,
    memory_map: Rc<MemoryMap>,
    device_id: DeviceId,

    overhead_size_bytes: usize,

    /// Slots to handle one or more requests simultaneously
    active_request_slots: RefCell<Vec<ActiveRequestSlot>>,

    /// Queue of active requests that haven't been sent yet
    pending_request_indices: RefCell<VecDeque<usize>>,

    /// Event to notify the port driver that there is a new request to handle
    new_request: Repeated<()>,

    /// Event to notify anyone waiting for a slot that one is now available
    slot_available: Repeated<()>,

    /// Give concurrent transfers fair turns to issue individual requests.
    request_arbiter: RequestArbiter,
}

impl LsuState {
    // Find a and allocate an available request slot.
    //
    // Will wait for one to be freed up if there are currently none available.
    async fn allocate_request_slot(&self) -> usize {
        loop {
            for (i, request) in self
                .active_request_slots
                .borrow_mut()
                .iter_mut()
                .enumerate()
            {
                if !request.in_use {
                    request.in_use = true;
                    debug!(self.entity ; "Allocate slot {i}");
                    return i;
                }
            }
            // None free - wait to be told there is one and try again
            self.slot_available.listen().await;
        }
    }

    // Create the memory access to be sent to the memory subsystem
    fn create_memory_access(
        &self,
        access_type: AccessType,
        access_size_bytes: usize,
        dst_addr: u64,
        request_slot_idx: usize,
    ) -> Result<MemoryAccess, SimError> {
        let overhead_size_bytes = self.overhead_size_bytes;

        // Use the slot index as the source address so that it can be routed
        // correctly on reply
        let src_addr = request_slot_idx as u64;

        let dst_device = self
            .memory_map
            .resolve_access(dst_addr, access_size_bytes)?;
        let src_device = self.device_id;

        Ok(MemoryAccess::new(
            &self.entity,
            access_type,
            access_size_bytes,
            dst_addr,
            src_addr,
            dst_device,
            src_device,
            overhead_size_bytes,
        ))
    }

    // Place the request in the specified slot and notify the port driver to
    // handle this request by putting the slot index in the pending queue.
    //
    // Returns the event that will be used to notify when the response returns.
    fn make_request_to_port_driver(
        &self,
        slot_idx: usize,
        access: MemoryAccess,
    ) -> Result<Repeated<()>, SimError> {
        let response_ready_event = {
            let mut guard = self.active_request_slots.borrow_mut();
            guard[slot_idx].request = Some(access);
            guard[slot_idx].response_ready_event.clone()
        };

        // Add the index to the queue of pending indices
        self.pending_request_indices
            .borrow_mut()
            .push_back(slot_idx);

        // Wake the port driver
        self.new_request.notify();

        Ok(response_ready_event)
    }

    fn handle_response_in_slot(&self, slot_idx: usize) -> SimResult {
        let mut guard = self.active_request_slots.borrow_mut();
        if guard[slot_idx].in_use {
            guard[slot_idx].in_use = false;
            debug!(self.entity ; "Release slot {slot_idx}");
            self.slot_available.notify();
        } else {
            return sim_error!("Response when slot {slot_idx} not in use.");
        }

        // Take and drop the response
        if guard[slot_idx].response.take().is_none() {
            return sim_error!("handle_response_in_slot called with response in slot {slot_idx}.");
        }

        // TODO: do something with the response

        Ok(())
    }

    // Take the next active request from the pending queue and drive it onto the
    // TX port
    async fn try_handle_next_active_request(
        &self,
        tx: &mut OutPort<MemoryAccess>,
    ) -> Result<bool, SimError> {
        let next_slot_idx = self.pending_request_indices.borrow_mut().pop_front();

        let handled_request = if let Some(slot_idx) = next_slot_idx {
            let request = {
                let mut guard = self.active_request_slots.borrow_mut();
                if !guard[slot_idx].in_use {
                    return sim_error!("Request for {slot_idx} sent when not in_use");
                }
                match guard[slot_idx].request.take() {
                    Some(request) => request,
                    None => return sim_error!("Request for {slot_idx} while request still None."),
                }
            };
            debug!(self.entity ; "Make memory access {request} for slot {slot_idx}");
            tx.put(request)?.await;
            true
        } else {
            false
        };
        Ok(handled_request)
    }

    fn put_response_in_active_request_slot(&self, response: MemoryAccess) -> SimResult {
        let idx = response.src_addr() as usize;
        let mut guard = self.active_request_slots.borrow_mut();
        match guard.get_mut(idx) {
            None => sim_error!("Invalid index '{idx}' in response"),
            Some(active_request) => {
                active_request.response = Some(response);
                active_request.response_ready_event.notify();
                Ok(())
            }
        }
    }
}

/// Each active request slot manages the request from the Processing Element
/// and ensures the corresponding response is handled.
struct ActiveRequestSlot {
    in_use: bool,
    request: Option<MemoryAccess>,
    response_ready_event: Repeated<()>,
    response: Option<MemoryAccess>,
}

impl Default for ActiveRequestSlot {
    fn default() -> Self {
        ActiveRequestSlot {
            in_use: false,
            request: None,
            response_ready_event: Repeated::new(()),
            response: None,
        }
    }
}

#[cfg(test)]
mod tests;
