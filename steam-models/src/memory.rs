// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_components::delay::Delay;
use steam_components::{port_rx, take_option};
use steam_engine::engine::Engine;
use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortStateResult};
use steam_engine::time::clock::Clock;
use steam_engine::traits::{Runnable, SimObject};
use steam_engine::types::{AccessType, SimError, SimResult};
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::trace;

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

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum CacheHintType {
    Allocate,
    NoAllocate,
}

pub trait MemoryRead {
    fn read(&self) -> Vec<u8>;
}

/// Trait implemented by all types that memory components support
pub trait AccessMemory {
    /// Return the destination address of this access
    fn dst_addr(&self) -> u64;

    /// Return the source address of this access
    fn src_addr(&self) -> u64;

    /// Return the size of the access in bytes
    fn num_bytes(&self) -> usize;

    /// What type of memory operation is this
    fn access_type(&self) -> AccessType;

    /// Returns the appropriate response for a request
    fn to_response(&self, mem: &impl MemoryRead) -> Self;

    /// Returns the requested caching behaviour of a request
    fn cache_hint(&self) -> CacheHintType;
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

#[derive(EntityDisplay)]
pub struct Memory<T>
where
    T: SimObject + AccessMemory,
{
    pub entity: Arc<Entity>,
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
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        config: MemoryConfig,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));

        let rx = InPort::new(&entity, "rx");

        let response_delay = Delay::new_and_register(
            engine,
            &entity,
            "delay",
            clock.clone(),
            spawner,
            config.delay_ticks,
        )?;

        // Create a local port to drive into the response delay
        let mut response_tx = OutPort::new(&entity, "response");
        response_tx.connect(response_delay.port_rx())?;

        let rc_self = Rc::new(Self {
            entity,
            clock,
            config,
            metrics: RefCell::new(MemoryMetrics::new()),
            response_delay,
            rx: RefCell::new(Some(rx)),
            response_tx: RefCell::new(Some(response_tx)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
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
            trace!(self.entity ; "Memory access {}", access);

            let begin = access.dst_addr();
            let access_bytes = access.num_bytes() as u64;
            let end = begin + access_bytes;

            let config = &self.config;
            assert!(
                begin >= config.base_address
                    && end < (config.base_address + config.capacity_bytes as u64),
                "Invalid memory access received"
            );

            match access.access_type() {
                AccessType::Read => {
                    self.metrics.borrow_mut().bytes_read += access.num_bytes();
                    let response = access.to_response(self);
                    response_tx.put(response)?.await;
                }
                AccessType::Write | AccessType::WriteNonPosted => {
                    self.metrics.borrow_mut().bytes_written += access.num_bytes();

                    if access.access_type() == AccessType::WriteNonPosted {
                        response_tx.put(access)?.await;
                    }
                }
                AccessType::Control => {
                    todo!("control handling")
                }
            }

            let ticks = access_bytes.div_ceil(config.bw_bytes_per_cycle as u64);
            self.clock.wait_ticks(ticks).await;
        }
    }
}

impl<T> MemoryRead for Memory<T>
where
    T: SimObject + AccessMemory,
{
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}
