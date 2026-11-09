// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::fmt::{self, Display};
use std::ops::RangeInclusive;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::delay::Delay;
use gwr_components::{port_rx, take_option};
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::time::compute_adjusted_value_and_rate;
use gwr_engine::traits::{Runnable, SimObject};
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;
use gwr_track::{build_aka, debug};

use crate::log_stats;
use crate::memory::traits::{AccessMemory, ReadMemory};

pub mod atomic;
pub mod memory_access;
pub mod memory_access_gen;
pub mod memory_map;
mod service;
pub mod traits;
pub use service::{
    MAX_MEMORY_SERVICE_REQUESTS, MemoryServiceLaneSnapshot, MemoryServiceSnapshot, payload_by_lane,
};

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum CacheHintType {
    Allocate,
    NoAllocate,
}

#[derive(Clone)]
pub struct MemoryConfig {
    base_address: u64,
    capacity_bytes: usize,
    bw_bytes_per_tick: usize,
    delay_ticks: usize,
    arrival_aware_service: Option<usize>,
    service_lanes: Option<(u64, Vec<u64>)>,
    atomic_service: Option<(u64, atomic::AtomicServiceConfig)>,
}

impl MemoryConfig {
    #[must_use]
    pub fn new(
        base_address: u64,
        capacity_bytes: usize,
        bw_bytes_per_tick: usize,
        delay_ticks: usize,
    ) -> Self {
        Self {
            base_address,
            capacity_bytes,
            bw_bytes_per_tick,
            delay_ticks,
            arrival_aware_service: None,
            service_lanes: None,
            atomic_service: None,
        }
    }

    /// Select the analytic payload server with a total live request budget.
    /// The budget includes queued, active and completed unpublished requests.
    /// Uses positive integer configured bytes/tick at the actual1ns clock;
    /// separate fixed latency is pipelined through bounded publication.
    /// Omission preserves legacy timing. No physical HBM controller is implied.
    #[must_use]
    pub fn with_arrival_aware_service(mut self, max_pending_requests: usize) -> Self {
        self.arrival_aware_service = Some(max_pending_requests);
        self
    }

    /// Declare caller-assumed lanes within the existing aggregate byte budget.
    /// Requires arrival-aware service and actual1ns at registration. Rates are
    /// supplied explicitly, never inferred by dividing a hardware peak.
    #[must_use]
    pub fn with_service_lanes(mut self, interleave_bytes: u64, bytes_per_tick: Vec<u64>) -> Self {
        self.service_lanes = Some((interleave_bytes, bytes_per_tick));
        self
    }

    /// Explicit cache-free atomic target. Rates/lanes/capacity must agree with
    /// the ordinary declaration; no additional byte budget is introduced.
    #[must_use]
    pub fn with_atomic_service(
        mut self,
        device: u64,
        service: atomic::AtomicServiceConfig,
    ) -> Self {
        self.atomic_service = Some((device, service));
        self
    }
    pub fn validate(&self) -> SimResult {
        self.address_range()?;
        if let Some((_, service)) = &self.atomic_service {
            service.validate()?;
            let lanes = self.service_lanes.as_ref().ok_or_else(|| {
                SimError("atomic target requires exact explicit ordinary lanes".into())
            })?;
            if self.arrival_aware_service != Some(service.capacity)
                || u64::try_from(self.bw_bytes_per_tick).ok() != Some(service.bytes_per_tick)
                || u64::try_from(self.delay_ticks).ok() != Some(service.delay_ticks)
                || lanes.0 != service.lane_interleave_bytes
                || lanes.1 != service.lane_bytes_per_tick
            {
                return Err(SimError(
                    "atomic target conflicts with ordinary service declaration".into(),
                ));
            }
        }
        if self.bw_bytes_per_tick == 0 {
            return sim_error!("bandwidth must be greater than zero");
        }
        if self
            .arrival_aware_service
            .is_some_and(|n| n == 0 || n > MAX_MEMORY_SERVICE_REQUESTS)
        {
            return sim_error!("memory service request budget must be within1..=4096");
        }
        if let Some((interleave, rates)) = &self.service_lanes {
            if self.arrival_aware_service.is_none() {
                return sim_error!("memory service lanes require arrival-aware service");
            }
            service::validate_lane_config(*interleave, rates, self.bw_bytes_per_tick)?;
        }
        Ok(())
    }

    /// Return the inclusive physical address range covered by this memory.
    pub fn address_range(&self) -> Result<RangeInclusive<u64>, SimError> {
        let capacity = u64::try_from(self.capacity_bytes)
            .map_err(|error| SimError(format!("capacity cannot be represented as u64: {error}")))?;

        if capacity == 0 {
            return sim_error!("capacity must be greater than zero");
        }

        let Some(last_address) = checked_last_address(self.base_address, capacity) else {
            return sim_error!(
                "address range starting at 0x{:x} with capacity {} bytes exceeds the physical address space",
                self.base_address,
                self.capacity_bytes
            );
        };

        Ok(self.base_address..=last_address)
    }
}

/// Return the inclusive last address of a non-empty byte range.
#[must_use]
pub fn checked_last_address(start: u64, size: u64) -> Option<u64> {
    start.checked_add(size.checked_sub(1)?)
}

#[derive(Clone, Default)]
pub struct MemoryStats {
    bytes_read: usize,
    bytes_written: usize,
}

pub struct MemoryStatsDisplay {
    prefix: String,
    time_now_ns: f64,
    bytes_read: usize,
    bytes_written: usize,
}

impl MemoryStatsDisplay {
    #[must_use]
    pub fn new(
        prefix: impl Into<String>,
        time_now_ns: f64,
        bytes_read: usize,
        bytes_written: usize,
    ) -> Self {
        Self {
            prefix: prefix.into(),
            time_now_ns,
            bytes_read,
            bytes_written,
        }
    }
}

impl Display for MemoryStatsDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (read_value, read_per_second) =
            compute_adjusted_value_and_rate(self.time_now_ns, self.bytes_read);
        let (write_value, write_per_second) =
            compute_adjusted_value_and_rate(self.time_now_ns, self.bytes_written);

        writeln!(f, "{}:", self.prefix)?;
        writeln!(
            f,
            "  Read: {} bytes, {read_value:.2}, {read_per_second:.2}/s",
            self.bytes_read
        )?;
        write!(
            f,
            "  Written: {} bytes, {write_value:.2}, {write_per_second:.2}/s",
            self.bytes_written
        )
    }
}

#[derive(EntityGet, EntityDisplay)]
pub struct Memory<T>
where
    T: SimObject + AccessMemory,
{
    entity: Rc<Entity>,
    clock: Clock,
    config: MemoryConfig,
    last_address: u64,
    stats: RefCell<MemoryStats>,

    response_delay: Option<Rc<Delay<T>>>,
    service: Option<Rc<service::Service<T>>>,
    atomic: Option<Rc<atomic::SharedMemoryAuthority<T>>>,
    spawner: Spawner,
    response_tx: RefCell<Option<OutPort<T>>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> Memory<T>
where
    T: SimObject + AccessMemory,
{
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: MemoryConfig,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, config)
    }

    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: MemoryConfig,
    ) -> Result<Rc<Self>, SimError> {
        let with_entity = |error| SimError(format!("{parent}::{name}: {error}"));
        config.validate().map_err(&with_entity)?;
        if config.arrival_aware_service.is_some() && clock.freq_mhz() != 1000.0 {
            return Err(with_entity(SimError(
                "arrival-aware memory requires an actual1ns clock".into(),
            )));
        }
        let last_address = config
            .address_range()
            .map(|range| *range.end())
            .map_err(with_entity)?;
        let entity = Rc::new(Entity::new(parent, name));

        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);

        let atomic = match &config.atomic_service {
            Some((device, service)) => Some(atomic::SharedMemoryAuthority::new(
                engine,
                clock,
                &entity,
                *device,
                config.base_address,
                u64::try_from(config.capacity_bytes).map_err(|e| SimError(e.to_string()))?,
                service.clone(),
            )?),
            None => None,
        };
        let (response_delay, response_tx, service) = if atomic.is_some() {
            (None, OutPort::new_with_renames(&entity, "tx", aka), None)
        } else if let Some(capacity) = config.arrival_aware_service {
            (
                None,
                OutPort::new_with_renames(&entity, "tx", aka),
                Some(service::Service::new_with_lanes(
                    capacity,
                    config.bw_bytes_per_tick,
                    config.delay_ticks,
                    config.service_lanes.as_ref(),
                )?),
            )
        } else {
            let response_delay_aka = build_aka!(aka, &entity, &[("tx", "tx")]);
            let delay = Delay::new_and_register_with_renames(
                engine,
                clock,
                &entity,
                "delay",
                Some(&response_delay_aka),
                config.delay_ticks,
            );
            let mut tx = OutPort::new(&entity, "response");
            tx.connect(delay.port_rx())
                .expect("Internal ports should connect without error");
            (Some(delay), tx, None)
        };

        let rc_self = Rc::new(Self {
            entity,
            clock: clock.clone(),
            config,
            last_address,
            stats: RefCell::new(MemoryStats::default()),
            response_delay,
            service,
            atomic,
            spawner: engine.spawner(),
            rx: RefCell::new(Some(rx)),
            response_tx: RefCell::new(Some(response_tx)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        if let Some(delay) = &self.response_delay {
            delay.connect_port_tx(port_state)
        } else {
            let state = port_state?;
            let receiver = state.in_port_entity.clone();
            self.response_tx
                .borrow_mut()
                .as_mut()
                .ok_or_else(|| SimError("memory response port already running".into()))?
                .connect(Ok(state))?;
            if let Some(atomic) = &self.atomic {
                atomic.bind_ordinary_output(receiver);
            }
            Ok(())
        }
    }

    /// Live bounded ownership of the opt-in pipeline; legacy mode has no
    /// ledger.
    #[must_use]
    pub fn service_snapshot(&self) -> Option<MemoryServiceSnapshot> {
        self.service.as_ref().map(|service| service.snapshot())
    }

    /// Ordered cumulative admitted payload per caller lane, absent when lanes
    /// are omitted. Not a completion or physical occupancy observation.
    #[must_use]
    pub fn service_lane_snapshot(&self) -> Option<Vec<MemoryServiceLaneSnapshot>> {
        self.service
            .as_ref()
            .and_then(|service| service.lane_snapshot())
    }

    pub fn atomic_target(&self) -> Option<Rc<atomic::AtomicTargetSender>> {
        self.atomic.as_ref().map(|a| a.sender())
    }
    pub fn atomic_control(&self) -> Option<atomic::SharedAtomicControl> {
        self.atomic.as_ref().map(|a| a.control())
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        let state = port_rx!(self.rx, state)?;
        if let Some(atomic) = &self.atomic {
            atomic.bind_ordinary_input(state.in_port_entity.clone());
        }
        Ok(state)
    }

    #[must_use]
    pub fn bytes_written(&self) -> usize {
        if let Some(atomic) = &self.atomic {
            return usize::try_from(atomic.snapshot().committed_write_bytes)
                .expect("service validates usize committed counters");
        }
        self.stats.borrow().bytes_written
    }

    /// Read-only separate witness; existing committed counters are unchanged.
    pub fn atomic_snapshot(&self) -> Option<atomic::AtomicSnapshot> {
        self.atomic.as_ref().map(|a| a.snapshot())
    }
    pub fn enable_atomic_witness(&self, limit: usize) -> SimResult {
        self.atomic
            .as_ref()
            .ok_or_else(|| SimError("memory has no explicit atomic service".into()))?
            .enable_witness(limit)
    }
    pub fn atomic_owner_windows(&self) -> Option<Vec<atomic::AtomicOwnerWindow>> {
        self.atomic.as_ref().and_then(|a| a.owner_windows())
    }

    #[must_use]
    pub fn bytes_read(&self) -> usize {
        if let Some(atomic) = &self.atomic {
            return usize::try_from(atomic.snapshot().committed_read_bytes)
                .expect("service validates usize committed counters");
        }
        self.stats.borrow().bytes_read
    }

    #[must_use]
    pub fn base_address(&self) -> u64 {
        self.config.base_address
    }

    #[must_use]
    pub fn capacity_bytes(&self) -> usize {
        self.config.capacity_bytes
    }

    pub fn dump_stats(&self, time_now_ns: f64) {
        let stats = MemoryStats {
            bytes_read: self.bytes_read(),
            bytes_written: self.bytes_written(),
        };
        log_stats(
            &self.entity,
            MemoryStatsDisplay::new(
                format!("Memory {}", self.entity.full_name()),
                time_now_ns,
                stats.bytes_read,
                stats.bytes_written,
            ),
        );
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Memory<T>
where
    T: SimObject + AccessMemory,
{
    async fn run(&self) -> SimResult {
        let mut rx = take_option!(self.rx);
        let mut response_tx = take_option!(self.response_tx);

        if let Some(authority) = &self.atomic {
            let worker = authority.clone();
            let clock = self.clock.clone();
            self.spawner
                .spawn(async move { worker.run_service(clock).await });
            let publisher = authority.clone();
            self.spawner
                .spawn(async move { publisher.run_ordinary_publisher(response_tx).await });
            let publisher = authority.clone();
            self.spawner
                .spawn(async move { publisher.run_typed_publisher().await });
            let issuer = authority.clone();
            self.spawner
                .spawn(async move { issuer.run_typed_issuer().await });
            let receiver = authority.clone();
            self.spawner
                .spawn(async move { receiver.run_typed_reply_receiver().await });
            return authority
                .clone()
                .run_intake(self.clock.clone(), rx, |access| access.to_response(self))
                .await;
        }
        if let Some(service) = &self.service {
            let worker = service.clone();
            let worker_clock = self.clock.clone();
            self.spawner
                .spawn(async move { worker.run_service(worker_clock).await });
            let publisher = service.clone();
            let publisher_clock = self.clock.clone();
            self.spawner
                .spawn(async move { publisher.run_publisher(publisher_clock, response_tx).await });
            loop {
                service.wait_capacity().await;
                let access = rx.start_get()?.await;
                let bytes = access.access_size_bytes();
                let size = u64::try_from(bytes).map_err(|e| SimError(e.to_string()))?;
                let end = checked_last_address(access.dst_addr(), size)
                    .ok_or_else(|| SimError("invalid memory payload range".into()))?;
                if access.dst_addr() < self.config.base_address || end > self.last_address {
                    return sim_error!("{}: out of bounds memory payload", self.entity);
                }
                let kind = access.access_type();
                let address = access.dst_addr();
                let read_payload = kind == AccessType::ReadRequest;
                let response = match kind {
                    AccessType::ReadRequest | AccessType::WriteNonPostedRequest => {
                        Some(access.to_response(self)?)
                    }
                    AccessType::WriteRequest => None,
                    other => {
                        return sim_error!(
                            "{}: unsupported memory service access {other}",
                            self.entity
                        );
                    }
                };
                let (read, write) = {
                    let stats = self.stats.borrow();
                    let read = if kind == AccessType::ReadRequest {
                        stats.bytes_read.checked_add(bytes)
                    } else {
                        Some(stats.bytes_read)
                    };
                    let write = if kind != AccessType::ReadRequest {
                        stats.bytes_written.checked_add(bytes)
                    } else {
                        Some(stats.bytes_written)
                    };
                    (
                        read.ok_or_else(|| SimError("memory read counter overflows".into()))?,
                        write.ok_or_else(|| SimError("memory write counter overflows".into()))?,
                    )
                };
                service.accept_payload(
                    self.clock.tick_now().tick(),
                    bytes,
                    response,
                    self.config.base_address,
                    address,
                    read_payload,
                )?;
                self.stats.borrow_mut().bytes_read = read;
                self.stats.borrow_mut().bytes_written = write;
                rx.finish_get();
            }
        }

        loop {
            let access = rx.get()?.await;
            debug!(self.entity ; "Memory access {}", access);

            let begin = access.dst_addr();
            let payload_bytes = access.access_size_bytes();
            let access_size = u64::try_from(payload_bytes).map_err(|error| {
                SimError(format!("{}: memory access size: {error}", self.entity))
            })?;
            let Some(end) = checked_last_address(begin, access_size) else {
                return sim_error!(
                    "{}: memory access at 0x{begin:x} with size {payload_bytes} has an invalid range",
                    self.entity
                );
            };

            if begin < self.config.base_address || end > self.last_address {
                return sim_error!(
                    "{}: out of bounds memory access received [0x{begin:x},0x{end:x}] not in [0x{:x},0x{:x}]",
                    self.entity,
                    self.config.base_address,
                    self.last_address,
                );
            }

            let access_type = access.access_type();
            match access_type {
                AccessType::ReadRequest => {
                    self.stats.borrow_mut().bytes_read += payload_bytes;
                    let response = access.to_response(self)?;
                    response_tx.put(response)?.await;
                }
                AccessType::WriteRequest => {
                    self.stats.borrow_mut().bytes_written += payload_bytes;
                }
                AccessType::WriteNonPostedRequest => {
                    self.stats.borrow_mut().bytes_written += payload_bytes;
                    let response = access.to_ack()?;
                    response_tx.put(response)?.await;
                }
                AccessType::ReadResponse
                | AccessType::WriteNonPostedResponse
                | AccessType::BarrierRequest
                | AccessType::BarrierResponse => {
                    return sim_error!("{}: unsupported {access_type} received", self.entity);
                }
                AccessType::Control => {
                    todo!("control handling")
                }
            }

            let ticks = payload_bytes.div_ceil(self.config.bw_bytes_per_tick) as u64;
            self.clock.wait_ticks(ticks)?.await;
        }
    }
}

impl<T> ReadMemory for Memory<T>
where
    T: SimObject + AccessMemory,
{
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}
