// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;
/// A memory device and related traits
use std::sync::Arc;

use steam_components::delay::Delay;
use steam_components::{connect_tx, port_rx, take_option};
use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortState};
use steam_engine::spawn_subcomponent;
use steam_engine::time::clock::Clock;
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::trace;

#[derive(Clone)]
pub struct MemoryConfig {
    base_address: u64,
    capacity_bytes: u64,
    bw_bytes_per_cycle: u64,
    delay_ticks: usize,
}

impl MemoryConfig {
    pub fn new(
        base_address: u64,
        capacity_bytes: u64,
        bw_bytes_per_cycle: u64,
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

#[derive(PartialEq)]
pub enum MemoryAccessType {
    Read,
    Write,
    WriteNonPosted,
}

pub enum CacheHintType {
    Allocate,
    NoAllocate,
}

pub trait MemoryRead {
    fn read(&self) -> Vec<u8>;
}

/// Trait implemented by all types that memory components support
pub trait MemoryAccess {
    /// Return the address of this access
    fn addr(&self) -> u64;

    /// Return the size of the access in bytes
    fn num_bytes(&self) -> u64;

    /// What type of memory operation is this
    fn access_type(&self) -> MemoryAccessType;

    /// Returns the appropriate response for a request
    fn to_response(&self, mem: &impl MemoryRead) -> Self;

    /// Returns the requested caching behaviour of a request
    fn cache_hint(&self) -> CacheHintType;
}

#[derive(Clone)]
pub struct MemoryMetrics {
    bytes_read: u64,
    bytes_written: u64,
}

impl MemoryMetrics {
    fn new() -> Self {
        Self {
            bytes_read: 0,
            bytes_written: 0,
        }
    }
}

struct MemoryState<T>
where
    T: SimObject + MemoryAccess,
{
    clock: Clock,
    config: MemoryConfig,
    metrics: RefCell<MemoryMetrics>,

    response_delay: RefCell<Option<Delay<T>>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> MemoryState<T>
where
    T: SimObject + MemoryAccess,
{
    fn new(entity: Arc<Entity>, clock: Clock, spawner: Spawner, config: MemoryConfig) -> Self {
        let response_delay =
            Delay::new(&entity, "delay", clock.clone(), spawner, config.delay_ticks);
        Self {
            clock,
            config,
            metrics: RefCell::new(MemoryMetrics::new()),
            response_delay: RefCell::new(Some(response_delay)),
            rx: RefCell::new(Some(InPort::new(entity))),
        }
    }
}

#[derive(Clone, EntityDisplay)]
pub struct Memory<T>
where
    T: SimObject + MemoryAccess,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    state: Rc<MemoryState<T>>,
}

impl<T> Memory<T>
where
    T: SimObject + MemoryAccess,
{
    pub fn new(
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        config: MemoryConfig,
    ) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        Self {
            entity: entity.clone(),
            spawner: spawner.clone(),
            state: Rc::new(MemoryState::new(entity, clock, spawner, config)),
        }
    }

    pub fn connect_port_tx(&self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.response_delay, connect_port_tx ; port_state);
    }

    pub fn port_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.rx, state)
    }

    pub async fn run(&self) -> SimResult {
        let rx = take_option!(self.state.rx);

        // Create a local port to drive into the response delay
        let mut response_tx = OutPort::new(self.entity.clone(), "response");
        response_tx.connect(port_rx!(self.state.response_delay, port_rx));

        spawn_subcomponent!(self.spawner ; self.state.response_delay);

        loop {
            let access = rx.get().await;
            trace!(self.entity ; "Memory access {}", access);

            let begin = access.addr();
            let access_bytes = access.num_bytes();
            let end = access.addr() + access_bytes;

            let config = &self.state.config;
            assert!(
                begin >= config.base_address && end < (config.base_address + config.capacity_bytes),
                "Invalid memory access received"
            );

            match access.access_type() {
                MemoryAccessType::Read => {
                    self.state.metrics.borrow_mut().bytes_read += access.num_bytes();
                    response_tx.put(access).await?;
                }
                MemoryAccessType::Write | MemoryAccessType::WriteNonPosted => {
                    self.state.metrics.borrow_mut().bytes_written += access.num_bytes();

                    if access.access_type() == MemoryAccessType::WriteNonPosted {
                        response_tx.put(access).await?;
                    }
                }
            };

            let ticks = access_bytes.div_ceil(config.bw_bytes_per_cycle);
            self.state.clock.wait_ticks(ticks).await;
        }
    }

    pub fn bytes_written(&self) -> u64 {
        self.state.metrics.borrow().bytes_written
    }

    pub fn bytes_read(&self) -> u64 {
        self.state.metrics.borrow().bytes_read
    }
}
