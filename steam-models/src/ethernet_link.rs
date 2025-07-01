// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Bi-directional link with two ends (a & b).
//!
//! Models the bi-directional pipelined connection provided by an ethernet link.
//!
//! # Ports
//!
//! This component has four ports:
//!  - Two [passive put ports](steam_engine::port::InPort): `rx_a`, `rx_b`,
//!  - Two [active put ports](steam_engine::port::OutPort): `tx_a`, `tx_b`,

use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_components::delay::Delay;
use steam_components::flow_controls::limiter::Limiter;
use steam_components::{connect_port, rc_limiter};
use steam_engine::engine::Engine;
use steam_engine::executor::Spawner;
use steam_engine::port::PortState;
use steam_engine::time::clock::Clock;
use steam_engine::traits::SimObject;
use steam_model_builder::{EntityDisplay, Runnable};
use steam_track::entity::Entity;

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
    #[must_use]
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
    ) -> Rc<Self> {
        let entity = Arc::new(Entity::new(parent, name));
        let limiter = rc_limiter!(clock.clone(), BITS_PER_TICK);
        let limiter_a = Limiter::new_and_register(engine, &entity, "limit_a", limiter.clone());
        let delay_a = Delay::new_and_register(
            engine,
            &entity,
            "a",
            clock.clone(),
            spawner.clone(),
            DELAY_TICKS,
        );
        connect_port!(limiter_a, tx => delay_a, rx);

        let limiter_b: Rc<Limiter<_>> =
            Limiter::new_and_register(engine, &entity, "limit_b", limiter.clone());
        let delay_b = Delay::new_and_register(engine, &entity, "b", clock, spawner, DELAY_TICKS);
        connect_port!(limiter_b, tx => delay_b, rx);

        let rc_self = Rc::new(Self {
            entity: entity.clone(),
            limiter_a,
            delay_a,
            limiter_b,
            delay_b,
        });
        engine.register(rc_self.clone());
        rc_self
    }

    pub fn set_delay(&self, delay: usize) {
        self.delay_a.set_delay(delay);
        self.delay_b.set_delay(delay);
    }

    pub fn connect_port_tx_a(&self, port_state: Rc<PortState<T>>) {
        self.delay_a.connect_port_tx(port_state);
    }

    pub fn connect_port_tx_b(&self, port_state: Rc<PortState<T>>) {
        self.delay_b.connect_port_tx(port_state);
    }

    #[must_use]
    pub fn port_rx_a(&self) -> Rc<PortState<T>> {
        self.limiter_a.port_rx()
    }

    #[must_use]
    pub fn port_rx_b(&self) -> Rc<PortState<T>> {
        self.limiter_b.port_rx()
    }
}
