// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::delay::Delay;
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
use gwr_track::{build_aka, debug};

use crate::memory::traits::{AccessMemory, ReadMemory};

pub mod cache;
pub mod memory_access;
pub mod memory_access_gen;
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

#[derive(Clone)]
pub struct MemoryMetrics {
    bytes_read: usize,
    bytes_written: usize,
}

impl MemoryMetrics {
    fn new() -> Self {
        Self {
            bytes_read: 0,
            bytes_written: 0,
        }
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
    metrics: RefCell<MemoryMetrics>,

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
        )?;

        // Create a local port to drive into the response delay
        let mut response_tx = OutPort::new(&entity, "response");
        response_tx.connect(response_delay.port_rx())?;

        let rc_self = Rc::new(Self {
            entity,
            clock: clock.clone(),
            config,
            metrics: RefCell::new(MemoryMetrics::new()),
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
        self.metrics.borrow().bytes_written
    }

    #[must_use]
    pub fn bytes_read(&self) -> usize {
        self.metrics.borrow().bytes_read
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Memory<T>
where
    T: SimObject + AccessMemory,
{
    async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        let response_tx = take_option!(self.response_tx);

        loop {
            let access = rx.get()?.await;
            debug!(self.entity ; "Memory access {}", access);

            let begin = access.dst_addr();
            let payload_bytes = access.access_size_bytes();
            let end = begin + payload_bytes as u64;

            let config = &self.config;
            assert!(
                begin >= config.base_address
                    && end < (config.base_address + config.capacity_bytes as u64),
                "Invalid memory access received"
            );

            let access_type = access.access_type();
            match access_type {
                AccessType::ReadRequest => {
                    self.metrics.borrow_mut().bytes_read += payload_bytes;
                    let response = access.to_response(self)?;
                    response_tx.put(response)?.await;
                }
                AccessType::WriteRequest => {
                    self.metrics.borrow_mut().bytes_written += payload_bytes;
                }
                AccessType::WriteNonPostedRequest => {
                    self.metrics.borrow_mut().bytes_written += payload_bytes;
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
