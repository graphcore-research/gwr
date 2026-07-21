// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::fmt::{self, Display};
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::delay::Delay;
use gwr_components::{port_rx, take_option};
use gwr_engine::engine::Engine;
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

pub mod cache;
pub mod memory_access;
pub mod memory_access_gen;
pub mod memory_map;
pub mod traits;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum CacheHintType {
    Allocate,
    NoAllocate,
}

#[derive(Clone)]
pub struct MemoryConfig {
    base_address: u64,
    capacity_bytes: usize,
    bw_bytes_per_cycle: usize,
    delay_ticks: usize,
}

impl MemoryConfig {
    #[must_use]
    pub fn new(
        base_address: u64,
        capacity_bytes: usize,
        bw_bytes_per_cycle: usize,
        delay_ticks: usize,
    ) -> Self {
        Self {
            base_address,
            capacity_bytes,
            bw_bytes_per_cycle,
            delay_ticks,
        }
    }
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
    stats: RefCell<MemoryStats>,

    response_delay: Rc<Delay<T>>,
    response_tx: RefCell<Option<OutPort<T>>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> Memory<T>
where
    T: SimObject + AccessMemory,
{
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: MemoryConfig,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));

        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);

        let response_delay_aka = build_aka!(aka, &entity, &[("tx", "tx")]);
        let response_delay = Delay::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "delay",
            Some(&response_delay_aka),
            config.delay_ticks,
        );

        // Create a local port to drive into the response delay
        let mut response_tx = OutPort::new(&entity, "response");
        response_tx
            .connect(response_delay.port_rx())
            .expect("Internal ports should connect without error");

        let rc_self = Rc::new(Self {
            entity,
            clock: clock.clone(),
            config,
            stats: RefCell::new(MemoryStats::default()),
            response_delay,
            rx: RefCell::new(Some(rx)),
            response_tx: RefCell::new(Some(response_tx)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: MemoryConfig,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, config)
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        self.response_delay.connect_port_tx(port_state)
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        port_rx!(self.rx, state)
    }

    #[must_use]
    pub fn bytes_written(&self) -> usize {
        self.stats.borrow().bytes_written
    }

    #[must_use]
    pub fn bytes_read(&self) -> usize {
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
        let stats = self.stats.borrow();
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

        loop {
            let access = rx.get()?.await;
            debug!(self.entity ; "Memory access {}", access);

            let begin = access.dst_addr();
            let payload_bytes = access.access_size_bytes();
            let end = begin + (payload_bytes as u64) - 1;

            let config = &self.config;
            assert!(
                begin >= config.base_address
                    && end < (config.base_address + config.capacity_bytes as u64),
                "Out of bounds memory access received [0x{begin:x},0x{end:x}] not in [0x{:x},0x{:x}]",
                config.base_address,
                config.base_address + config.capacity_bytes as u64
            );

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
                    let response = access.to_response(self)?;
                    response_tx.put(response)?.await;
                }
                AccessType::ReadResponse | AccessType::WriteNonPostedResponse => {
                    return sim_error!("{}: unsupported {access_type} received", self.entity);
                }
                AccessType::Control => {
                    todo!("control handling")
                }
            }

            let ticks = payload_bytes.div_ceil(config.bw_bytes_per_cycle) as u64;
            self.clock.wait_ticks(ticks).await;
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
