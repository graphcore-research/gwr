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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use steam_components::delay::Delay;
use steam_components::flow_controls::limiter::Limiter;
use steam_components::{borrow_option, connect_port, connect_tx, port_rx, rc_limiter};
use steam_engine::executor::Spawner;
use steam_engine::port::PortState;
use steam_engine::spawn_subcomponent;
use steam_engine::time::clock::Clock;
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;

// Default values for an Ethernet Link
pub const DELAY_TICKS: usize = 500;
pub const BITS_PER_TICK: usize = 100;

/// Bi-directtional link with two ends (a & b)
struct EthernetLinkState<T>
where
    T: SimObject,
{
    limiter_a: RefCell<Option<Limiter<T>>>,
    delay_a: RefCell<Option<Delay<T>>>,
    limiter_b: RefCell<Option<Limiter<T>>>,
    delay_b: RefCell<Option<Delay<T>>>,
}

impl<T> EthernetLinkState<T>
where
    T: SimObject,
{
    fn new(entity: Arc<Entity>, clock: Clock, spawner: Spawner) -> Self {
        let limiter = rc_limiter!(clock.clone(), BITS_PER_TICK);
        let limiter_a = Limiter::new(&entity, "limit_a", limiter.clone());
        let delay_a = Delay::new(&entity, "a", clock.clone(), spawner.clone(), DELAY_TICKS);
        connect_port!(limiter_a, tx => delay_a, rx);

        let limiter_b: Limiter<_> = Limiter::new(&entity, "limit_b", limiter.clone());
        let delay_b = Delay::new(&entity, "b", clock, spawner, DELAY_TICKS);
        connect_port!(limiter_b, tx => delay_b, rx);

        Self {
            limiter_a: RefCell::new(Some(limiter_a)),
            delay_a: RefCell::new(Some(delay_a)),
            limiter_b: RefCell::new(Some(limiter_b)),
            delay_b: RefCell::new(Some(delay_b)),
        }
    }
}

#[derive(Clone, EntityDisplay)]
pub struct EthernetLink<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    state: Rc<EthernetLinkState<T>>,
}

impl<T> EthernetLink<T>
where
    T: SimObject,
{
    pub fn new(parent: &Arc<Entity>, name: &str, clock: Clock, spawner: Spawner) -> Self {
        let entity = Arc::new(Entity::new(parent, name));

        Self {
            entity: entity.clone(),
            spawner: spawner.clone(),
            state: Rc::new(EthernetLinkState::new(entity, clock, spawner)),
        }
    }

    pub fn set_delay(&self, delay: usize) {
        borrow_option!(self.state.delay_a).set_delay(delay);
        borrow_option!(self.state.delay_b).set_delay(delay);
    }

    pub fn connect_port_tx_a(&mut self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.delay_a, connect_port_tx ; port_state);
    }

    pub fn connect_port_tx_b(&mut self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.delay_b, connect_port_tx ; port_state);
    }

    pub fn port_rx_a(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.limiter_a, port_rx)
    }

    pub fn port_rx_b(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.limiter_b, port_rx)
    }

    pub async fn run(&self) -> SimResult {
        spawn_subcomponent!(self.spawner ; self.state.limiter_a);
        spawn_subcomponent!(self.spawner ; self.state.delay_a);
        spawn_subcomponent!(self.spawner ; self.state.limiter_b);
        spawn_subcomponent!(self.spawner ; self.state.delay_b);
        Ok(())
    }
}
