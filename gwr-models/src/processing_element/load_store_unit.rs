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

use std::cell::{Cell, RefCell};
use std::cmp::min;
use std::collections::VecDeque;
use std::fmt;
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
use gwr_engine::time::clock::{Clock, phase};
use gwr_engine::traits::{Event, Routable, Runnable};
use gwr_engine::types::{AccessType, DeviceId, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::debug;
use gwr_track::entity::{Entity, EntityGroup};
use gwr_track::tracker::aka::Aka;

use super::packet_arbiter::PacketArbiter;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::processing_element::{ActivityLanes, ProcessingElementConfig};

/// Packet issue observations on the real LSU request path. Wait totals are
/// overlapping producer lifetimes and must not be interpreted as elapsed time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LsuIssueSnapshot {
    pub issued_read_packets: u64,
    pub issued_write_packets: u64,
    pub arbitration_wait_ticks: u64,
    pub credit_wait_ticks: u64,
    pub peak_outstanding_packets: usize,
    pub outstanding_packets: usize,
}

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
    atomic_request_receiver: RefCell<Option<Rc<Entity>>>,
    atomic_reply_receiver: RefCell<Option<Rc<Entity>>>,
}

impl fmt::Debug for LoadStoreUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
            serialiser: PacketArbiter::default(),
            issue: RefCell::new(LsuIssueSnapshot::default()),
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
            atomic_request_receiver: RefCell::new(None),
            atomic_reply_receiver: RefCell::new(None),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<MemoryAccess>) -> SimResult {
        let state = port_state?;
        let receiver = state.in_port_entity.clone();
        let port_state = Ok(state);
        connect_tx!(self.tx, connect ; port_state)?;
        *self.atomic_request_receiver.borrow_mut() = Some(receiver);
        Ok(())
    }

    pub fn port_rx(&self) -> PortStateResult<MemoryAccess> {
        let state = port_rx!(self.rx, state)?;
        *self.atomic_reply_receiver.borrow_mut() = Some(state.in_port_entity.clone());
        Ok(state)
    }
    pub(crate) fn validate_atomic_direct_target(
        &self,
        target: &crate::memory::atomic::AtomicTargetSender,
    ) -> SimResult {
        target.validate_ordinary_port_pair(
            self.atomic_request_receiver.borrow().as_ref(),
            self.atomic_reply_receiver.borrow().as_ref(),
        )
    }

    /// Check every physical byte touched by a view is mapped. Untouched
    /// strided holes need not be mapped; packed elements count unique bytes.
    /// This is independent of request issuing and allocates no request slots.
    pub(crate) fn validate_mapped_view(
        &self,
        view: &crate::processing_element::tensors::TensorView,
    ) -> SimResult {
        for range in self.state.memory_map.logical_ranges() {
            if view.num_access_bytes_in(range.clone()) != 0 {
                self.state
                    .memory_map
                    .validate_fetch_size(range.start as u64, self.max_access_size_bytes)?;
            }
        }
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

    /// Validate the full ordinary packets as well as logical touched bytes.
    /// Direct model users can supply maps that lack platform YAML alignment
    /// guarantees.
    pub(crate) fn validate_aligned_view(
        &self,
        view: &crate::processing_element::tensors::TensorView,
    ) -> SimResult {
        self.validate_mapped_view(view)?;
        let fetch = self.max_access_size_bytes as u128;
        for range in view.address_ranges() {
            let mut cursor = range.start;
            while cursor < range.end {
                let region = self
                    .state
                    .memory_map
                    .logical_range_at(
                        u64::try_from(cursor)
                            .map_err(|_| SimError("aligned LSU address overflow".into()))?,
                    )
                    .ok_or_else(|| SimError("unmapped aligned LSU packet".into()))?;
                let end = range.end.min(region.end);
                let packet_start = cursor / fetch * fetch;
                let packet_end = (end - 1) / fetch * fetch + fetch;
                if packet_start < region.start || packet_end > region.end {
                    return Err(SimError(
                        "aligned LSU packet crosses memory map/device boundary".into(),
                    ));
                }
                cursor = end;
            }
        }
        Ok(())
    }

    pub(crate) fn validate_program_views(
        &self,
        config: &super::task::ProgramTaskConfig,
    ) -> SimResult {
        for view in config.inputs.iter().chain(&config.outputs) {
            self.validate_mapped_view(view)?;
        }
        Ok(())
    }

    pub(crate) fn validate_source_target(
        &self,
        issues: impl IntoIterator<Item = super::runner::streamed_matrix::SourceVectorIssue>,
        device: u64,
    ) -> SimResult {
        let issues = issues.into_iter().collect::<Vec<_>>();
        self.validate_source_issues(issues.iter().copied())?;
        for issue in issues {
            let resolved = self.state.memory_map.resolve_access(issue.address, 16)?;
            if resolved.device.0 != device {
                return sim_error!("source vector maps to a different actual shared memory target");
            }
        }
        Ok(())
    }
    pub(crate) fn validate_source_issues(
        &self,
        issues: impl IntoIterator<Item = super::runner::streamed_matrix::SourceVectorIssue>,
    ) -> SimResult {
        let issues = issues.into_iter().collect::<Vec<_>>();
        if self.max_access_size_bytes < 16 {
            return sim_error!("LSU cannot issue original 16B source vectors");
        }
        for issue in &issues {
            self.state
                .memory_map
                .resolve_access(issue.address, 16)
                .map_err(|error| SimError(format!("source 16B vector: {error}")))?;
        }
        Ok(())
    }

    /// Explicit source issues retain source order and duplicates. Validate the
    /// entire plan before issuing, including each full 16B packet's map/device.
    /// Existing range path supplies real bounded slots and actual reply ACKs.
    pub(crate) async fn access_source_issues(
        &self,
        issues: impl IntoIterator<Item = super::runner::streamed_matrix::SourceVectorIssue>,
        activity_lanes: &Rc<RefCell<ActivityLanes>>,
        activity_name: &str,
        group: &Rc<EntityGroup>,
        storage: Rc<dyn std::any::Any>,
        source_stats: Rc<RefCell<super::ProcessingElementStats>>,
    ) -> SimResult {
        let issues = issues.into_iter().collect::<Vec<_>>();
        self.validate_source_issues(issues.iter().copied())?;
        self.access_ranges_with_storage(
            AccessType::ReadRequest,
            issues.into_iter().map(|issue| issue.range()),
            activity_lanes,
            activity_name,
            group,
            Some(storage),
            Some(source_stats),
            Packetisation::ExplicitSourceVectors,
            #[cfg(feature = "functional")]
            None,
        )
        .await
    }

    /// Transfer validated physical ranges in aligned, fixed-size ordinary
    /// accesses.
    pub(crate) async fn access_ranges(
        &self,
        access_type: AccessType,
        ranges: impl IntoIterator<Item = Range<u128>>,
        activity_lanes: &Rc<RefCell<ActivityLanes>>,
        activity_name: &str,
        group: &Rc<EntityGroup>,
    ) -> SimResult {
        self.access_ranges_with_storage(
            access_type,
            ranges,
            activity_lanes,
            activity_name,
            group,
            None,
            None,
            Packetisation::OrdinaryAligned,
            #[cfg(feature = "functional")]
            None,
        )
        .await
    }
    #[cfg(feature = "functional")]
    pub(crate) fn validate_functional_mapping(&self) -> SimResult {
        if self.state.memory_map.has_striping() {
            return sim_error!("functional mode does not support striped memory maps");
        }
        Ok(())
    }

    #[cfg(feature = "functional")]
    pub(crate) async fn access_ranges_functional(
        &self,
        access_type: AccessType,
        ranges: impl IntoIterator<Item = Range<u128>>,
        activity_lanes: &Rc<RefCell<ActivityLanes>>,
        activity_name: &str,
        group: &Rc<EntityGroup>,
        context: &crate::memory::FunctionalContext,
    ) -> SimResult {
        self.validate_functional_mapping()?;
        self.access_ranges_with_storage(
            access_type,
            ranges,
            activity_lanes,
            activity_name,
            group,
            None,
            None,
            Packetisation::OrdinaryAligned,
            Some(context),
        )
        .await
    }

    /// One bounded DMA chunk, using the same real read and write requests.
    pub(crate) async fn transfer_chunk(
        &self,
        source: Range<u128>,
        destination: Range<u128>,
        lanes: &super::activity::ProcessingElementActivityLanes,
        name: &str,
        group: &Rc<EntityGroup>,
        #[cfg(feature = "functional")] context: Option<&crate::memory::FunctionalContext>,
    ) -> SimResult {
        #[cfg(feature = "functional")]
        if let Some(context) = context {
            self.access_ranges_functional(
                AccessType::ReadRequest,
                [source.clone()],
                &lanes.lsu_read,
                name,
                group,
                context,
            )
            .await?;
            context.copy_bytes(
                source.start as u64,
                destination.start as u64,
                (source.end - source.start) as usize,
            )?;
            return self
                .access_ranges_functional(
                    AccessType::WriteNonPostedRequest,
                    [destination],
                    &lanes.lsu_write,
                    name,
                    group,
                    context,
                )
                .await;
        }
        self.access_ranges(
            AccessType::ReadRequest,
            [source],
            &lanes.lsu_read,
            name,
            group,
        )
        .await?;
        self.access_ranges(
            AccessType::WriteNonPostedRequest,
            [destination],
            &lanes.lsu_write,
            name,
            group,
        )
        .await
    }

    pub(crate) fn issue_snapshot(&self) -> LsuIssueSnapshot {
        let mut s = self.state.issue.borrow().clone();
        s.outstanding_packets = self
            .state
            .active_request_slots
            .borrow()
            .iter()
            .filter(|slot| slot.in_use)
            .count();
        s
    }

    /// Retain a buffer reservation through actual completion of every issued
    /// ordinary aligned access, including when the requesting future is
    /// dropped.
    pub(crate) async fn access_ranges_retaining(
        &self,
        access_type: AccessType,
        ranges: impl IntoIterator<Item = Range<u128>>,
        activity_lanes: &Rc<RefCell<ActivityLanes>>,
        activity_name: &str,
        group: &Rc<EntityGroup>,
        custody: Rc<dyn std::any::Any>,
    ) -> SimResult {
        self.access_ranges_with_storage(
            access_type,
            ranges,
            activity_lanes,
            activity_name,
            group,
            Some(custody),
            None,
            Packetisation::OrdinaryAligned,
            #[cfg(feature = "functional")]
            None,
        )
        .await
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Keep storage ownership and completion guards in the same access scope"
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "Access metadata and storage lifetimes are independent parts of one request"
    )]
    async fn access_ranges_with_storage(
        &self,
        access_type: AccessType,
        ranges: impl IntoIterator<Item = Range<u128>>,
        activity_lanes: &Rc<RefCell<ActivityLanes>>,
        activity_name: &str,
        group: &Rc<EntityGroup>,
        storage: Option<Rc<dyn std::any::Any>>,
        source_stats: Option<Rc<RefCell<super::ProcessingElementStats>>>,
        packetisation: Packetisation,
        #[cfg(feature = "functional")] functional: Option<&crate::memory::FunctionalContext>,
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
                    // Packet routing uses its first byte. Never send bytes
                    // owned by the next device in that
                    // packet, even for adjacent maps.
                    let resolved =
                        self.state
                            .memory_map
                            .resolve(access_address)
                            .ok_or_else(|| {
                                SimError(format!("Unmapped LSU address {access_address:#x}"))
                            })?;
                    let region_remaining = resolved.contiguous_bytes;
                    let (packet_address, access_size_bytes, covered_bytes) = if packetisation
                        == Packetisation::ExplicitSourceVectors
                    {
                        // Explicit source vectors retain their declared packet
                        // size.
                        let bytes = min(
                            min(self.max_access_size_bytes, bytes_remaining),
                            usize::try_from(region_remaining).unwrap_or(usize::MAX),
                        );
                        (access_address, bytes, bytes)
                    } else {
                        // Main's ordinary LSU path services aligned full-size
                        // accesses, while advancing by
                        // the logical tensor bytes they cover.
                        let offset = (access_address % self.max_access_size_bytes as u64) as usize;
                        (
                            access_address - offset as u64,
                            self.max_access_size_bytes,
                            min(self.max_access_size_bytes - offset, bytes_remaining),
                        )
                    };
                    let before = self.clock.tick_now().tick();
                    let packet_turn = self.state.serialiser.request().await;
                    let acquired = self.clock.tick_now().tick();
                    let request_slot_idx = self.state.allocate_request_slot().await;
                    // Both ordinary and retained callers own only acquired
                    // credits across suspension until the worker takes over.
                    let mut preissue_slot = RequestSlotGuard {
                        state: self.state.clone(),
                        slot: request_slot_idx,
                        armed: true,
                    };
                    {
                        let mut s = self.state.issue.borrow_mut();
                        s.arbitration_wait_ticks = s
                            .arbitration_wait_ticks
                            .checked_add(acquired - before)
                            .ok_or_else(|| SimError("LSU wait counter overflow".into()))?;
                        s.credit_wait_ticks = s
                            .credit_wait_ticks
                            .checked_add(self.clock.tick_now().tick() - acquired)
                            .ok_or_else(|| SimError("LSU wait counter overflow".into()))?;
                    }
                    if activity_guard.is_none() {
                        // Reuse a lane released in this tick before allocating
                        // one.
                        self.clock.wait_phase(phase::END).await;
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

                    #[cfg(feature = "functional")]
                    let access = {
                        let mut access = access;
                        access.functional = functional
                            .map(|context| {
                                context.packet(access_type, access_address, covered_bytes)
                            })
                            .transpose()?;
                        access
                    };
                    {
                        let state = self.state.clone();
                        let completion = completion.clone();
                        completion.request_started()?;
                        let storage = storage.clone();
                        let source_stats = source_stats.clone();
                        self.spawner.spawn(async move {
                            let _storage = storage; // survives caller cancellation through actual LSU reply ACK
                            let response_ready_event =
                                state.make_request_to_port_driver(request_slot_idx, access)?;
                            // Actual queue/port worker now owns the slot. It
                            // will be released only
                            // by handling the actual reply.
                            preissue_slot.disarm();
                            drop(packet_turn);
                            response_ready_event.listen().await;
                            let result = state.handle_response_in_slot(request_slot_idx);
                            if result.is_ok()
                                && let Some(stats) = source_stats
                            {
                                let mut stats = stats.borrow_mut();
                                stats.read_bytes = stats
                                    .read_bytes
                                    .checked_add(access_size_bytes)
                                    .ok_or_else(|| {
                                        SimError("completed source PE read counter overflow".into())
                                    })?;
                            }
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

    /// Ensure that the LSU is only used by one `Task` at a time
    serialiser: PacketArbiter,
    issue: RefCell<LsuIssueSnapshot>,
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

        let resolved = self
            .memory_map
            .resolve_access(dst_addr, access_size_bytes)?;
        let dst_device = resolved.device;
        let dst_addr = resolved.physical_address;
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
        {
            let mut s = self.issue.borrow_mut();
            let counter = if access.access_type() == AccessType::ReadRequest {
                &mut s.issued_read_packets
            } else {
                &mut s.issued_write_packets
            };
            *counter = counter
                .checked_add(1)
                .ok_or_else(|| SimError("LSU issued packet counter overflow".into()))?;
            s.peak_outstanding_packets = s.peak_outstanding_packets.max(
                self.active_request_slots
                    .borrow()
                    .iter()
                    .filter(|slot| slot.in_use)
                    .count(),
            );
        }
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
        let response = guard[slot_idx].response.take();
        if response.is_none() {
            return sim_error!("handle_response_in_slot called with response in slot {slot_idx}.");
        }

        #[cfg(feature = "functional")]
        if let Some(response) = response
            && let Some(packet) = &response.functional
        {
            packet.receive(response.access_type())?;
        }
        #[cfg(not(feature = "functional"))]
        drop(response);

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Packetisation {
    OrdinaryAligned,
    ExplicitSourceVectors,
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
#[cfg(test)]
impl LoadStoreUnit {
    pub(crate) fn source_slots_for_test(&self) -> Vec<bool> {
        self.state
            .active_request_slots
            .borrow()
            .iter()
            .map(|slot| slot.in_use)
            .collect()
    }
}

#[cfg(test)]
#[path = "load_store_unit_tests.rs"]
mod custody_tests;
