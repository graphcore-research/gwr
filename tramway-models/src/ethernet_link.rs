// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Bi-directional link with two ends (a & b).
//!
//! Models the bi-directional pipelined connection provided by an ethernet link.
//!
//! # Ports
//!
//! This component has four ports:
//!  - Two [passive put ports](tramway_engine::port::InPort): `rx_a`, `rx_b`,
//!  - Two [active put ports](tramway_engine::port::OutPort): `tx_a`, `tx_b`,

use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use tramway_components::delay::Delay;
use tramway_components::flow_controls::limiter::Limiter;
use tramway_components::{connect_port, rc_limiter};
use tramway_engine::engine::Engine;
use tramway_engine::executor::Spawner;
use tramway_engine::port::PortStateResult;
use tramway_engine::time::clock::Clock;
use tramway_engine::traits::SimObject;
use tramway_engine::types::{SimError, SimResult};
use tramway_model_builder::{EntityDisplay, Runnable};
use tramway_track::entity::Entity;

// Default values for an Ethernet Link
pub const DELAY_TICKS: usize = 500;
pub const BITS_PER_TICK: usize = 100;

#[derive(EntityDisplay, Runnable)]
pub struct EthernetLink<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    limiter_a: Rc<Limiter<T>>,
    delay_a: Rc<Delay<T>>,
    limiter_b: Rc<Limiter<T>>,
    delay_b: Rc<Delay<T>>,
}

impl<T> EthernetLink<T>
where
    T: SimObject,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));
        let limiter = rc_limiter!(clock.clone(), BITS_PER_TICK);
        let limiter_a = Limiter::new_and_register(engine, &entity, "limit_a", limiter.clone())?;
        let delay_a = Delay::new_and_register(
            engine,
            &entity,
            "a",
            clock.clone(),
            spawner.clone(),
            DELAY_TICKS,
        )?;
        connect_port!(limiter_a, tx => delay_a, rx)?;

        let limiter_b: Rc<Limiter<_>> =
            Limiter::new_and_register(engine, &entity, "limit_b", limiter.clone())?;
        let delay_b = Delay::new_and_register(engine, &entity, "b", clock, spawner, DELAY_TICKS)?;
        connect_port!(limiter_b, tx => delay_b, rx)?;

        let rc_self = Rc::new(Self {
            entity: entity.clone(),
            limiter_a,
            delay_a,
            limiter_b,
            delay_b,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn set_delay(&self, delay: usize) -> SimResult {
        self.delay_a.set_delay(delay)?;
        self.delay_b.set_delay(delay)
    }

    pub fn connect_port_tx_a(&self, port_state: PortStateResult<T>) -> SimResult {
        self.delay_a.connect_port_tx(port_state)
    }

    pub fn connect_port_tx_b(&self, port_state: PortStateResult<T>) -> SimResult {
        self.delay_b.connect_port_tx(port_state)
    }

    pub fn port_rx_a(&self) -> PortStateResult<T> {
        self.limiter_a.port_rx()
    }

    pub fn port_rx_b(&self) -> PortStateResult<T> {
        self.limiter_b.port_rx()
    }
}
