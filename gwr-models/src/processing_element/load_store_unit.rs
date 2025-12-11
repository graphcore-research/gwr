// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A model of a Load/Store Unit for a Processing Element (PE).
//!
//! The LSU supports a user-defined number of outstanding operations
//! and can be shared by multiple simultaneous tasks within the PE.

//! # Ports
//!
//! The LSU uses:
//!  - One [input port](gwr_engine::port::InPort): `rx`
//!  - One [output port](gwr_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::cmp::min;
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::{connect_tx, port_rx};
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, Runnable};
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_resources::Resource;
use gwr_track::debug;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;

use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::{DeviceId, MemoryMap};
use crate::memory::traits::AccessMemory;
use crate::processing_element::ProcessingElementConfig;

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

struct LsuState {
    entity: Rc<Entity>,
    memory_map: Rc<MemoryMap>,
    device_id: DeviceId,

    overhead_size_bytes: usize,

    /// How much SRAM does this PE have in total
    sram_bytes: usize,

    /// Slots to handle one or more requests simultaneously
    active_request_slots: RefCell<Vec<ActiveRequestSlot>>,

    /// Queue of active requests that haven't been sent yet
    pending_request_indices: RefCell<VecDeque<usize>>,

    /// Event to notify the port driver that there is a new request to handle
    new_request: Repeated<()>,

    /// Event to notify anyone waiting for a slot that one is now available
    slot_available: Repeated<()>,

    /// Ensure that the LSU is only used by one `Task` at a time
    serialiser: Resource,
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

        // Use the slot index as the source address so that it can be routed correctly
        // on reply
        let src_addr = request_slot_idx as u64;

        let dst_device = match self.memory_map.lookup(dst_addr) {
            Some((dst_device, _)) => dst_device,
            None => return sim_error!("0x{dst_addr:x} not mapped"),
        };
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

    // Place the request in the specified slot and notify the port driver to handle
    // this request by putting the slot index in the pending queue.
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
        self.new_request.notify()?;

        Ok(response_ready_event)
    }

    fn handle_response_in_slot(&self, slot_idx: usize) -> SimResult {
        let mut guard = self.active_request_slots.borrow_mut();
        if guard[slot_idx].in_use {
            guard[slot_idx].in_use = false;
            debug!(self.entity ; "Release slot {slot_idx}");
            self.slot_available.notify()?;
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

    // Take the next active request from the pending queue and drive it onto the TX
    // port
    async fn try_handle_next_active_request(
        &self,
        tx: &OutPort<MemoryAccess>,
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
                active_request.response_ready_event.notify()?;
                Ok(())
            }
        }
    }
}

#[derive(EntityGet, EntityDisplay)]
pub struct LoadStoreUnit {
    entity: Rc<Entity>,
    spawner: Spawner,

    /// Max bytes the LSU can request at once
    max_access_size_bytes: usize,

    rx: RefCell<Option<InPort<MemoryAccess>>>,
    tx: RefCell<Option<OutPort<MemoryAccess>>>,

    state: Rc<LsuState>,
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
        let entity = Rc::new(Entity::new(parent, "lsu"));
        let tx = OutPort::new_with_renames(&entity, "tx", aka);
        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);

        let num_active_requests = pe_config.num_active_requests;
        let max_access_size_bytes = pe_config.lsu_access_bytes;
        let overhead_size_bytes = pe_config.overhead_size_bytes;
        let sram_bytes = pe_config.sram_bytes;
        let mut requests = Vec::with_capacity(num_active_requests);
        for _ in 0..num_active_requests {
            requests.push(ActiveRequestSlot::default());
        }

        let state = LsuState {
            entity: entity.clone(),
            memory_map: memory_map.clone(),
            device_id,
            overhead_size_bytes,
            sram_bytes,
            active_request_slots: RefCell::new(requests),
            pending_request_indices: RefCell::new(VecDeque::new()),
            new_request: Repeated::new(()),
            slot_available: Repeated::new(()),
            serialiser: Resource::new(1),
        };
        let spawner = engine.spawner();
        let rc_self = Rc::new(Self {
            entity,
            spawner,
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

    /// Perform a memory access
    ///
    /// This will break a larger request down into requests of the maximum size
    /// permitted by the LSU
    pub async fn do_access(
        &self,
        access_type: AccessType,
        access_size_bytes: usize,
        dst_addr: u64,
    ) -> SimResult {
        if access_size_bytes > self.state.sram_bytes {
            return sim_error!(
                "PE cannot do memory access of {access_size_bytes} as it only has SRAM with {} bytes.",
                self.state.sram_bytes
            );
        }

        let mut bytes_remaining = access_size_bytes;
        let mut access_address = dst_addr;

        // Ensure only one load/store Task uses the LSU at a time
        self.state.serialiser.request().await;

        loop {
            if bytes_remaining == 0 {
                break;
            }

            // Wait until there is a request slot available
            let request_slot_idx = self.state.allocate_request_slot().await;
            let access_size_bytes = min(self.max_access_size_bytes, bytes_remaining);
            let access = self.state.create_memory_access(
                access_type,
                access_size_bytes,
                access_address,
                request_slot_idx,
            )?;

            {
                // Spawn off a handler for the request
                let state = self.state.clone();
                self.spawner.spawn(async move {
                    let response_ready_event =
                        state.make_request_to_port_driver(request_slot_idx, access)?;

                    // Wait for response to be received to slot
                    response_ready_event.listen().await;
                    state.handle_response_in_slot(request_slot_idx)
                });
            }

            bytes_remaining -= access_size_bytes;
            access_address += access_size_bytes as u64;
        }

        // Allow another load/store Task to start
        self.state.serialiser.release().await?;

        Ok(())
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

async fn run_rx(state: Rc<LsuState>, rx: InPort<MemoryAccess>) -> SimResult {
    loop {
        let response = rx.get()?.await;
        state.put_response_in_active_request_slot(response)?;
    }
}

async fn run_tx(state: Rc<LsuState>, tx: OutPort<MemoryAccess>) -> SimResult {
    loop {
        if !state.try_handle_next_active_request(&tx).await? {
            // Nothing was handled. Wait until the queue is changed.
            state.new_request.listen().await;
        }
    }
}
