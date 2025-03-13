// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This is an example component that will register a vector of subcomponents.
//!
//! The `main.rs` in this folder shows how it can be used.
//!
//! # Ports
//!
//! This component has four ports
//!  - Two [input port](steam_engine::port::InPort): `rx_a`, `rx_b`
//!  - Two [output port](steam_engine::port::OutPort): `tx_a`, `tx_b`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use steam_components::store::Store;
use steam_components::{connect_tx, port_rx};
use steam_engine::executor::Spawner;
use steam_engine::port::PortState;
use steam_engine::spawn_subcomponent;
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;

pub struct ScramblerState<T>
where
    T: SimObject,
{
    scramble: bool,
    buffer_a: RefCell<Option<Store<T>>>,
    buffer_b: RefCell<Option<Store<T>>>,
}

impl<T> ScramblerState<T>
where
    T: SimObject,
{
    pub fn new(entity: Arc<Entity>, spawner: Spawner, scramble: bool) -> Self {
        let buffer_a = Store::new(&entity, "buffer_a", spawner.clone(), 1);
        let buffer_b = Store::new(&entity, "buffer_b", spawner, 1);
        Self {
            scramble,
            buffer_a: RefCell::new(Some(buffer_a)),
            buffer_b: RefCell::new(Some(buffer_b)),
        }
    }
}

#[derive(Clone, EntityDisplay)]
pub struct Scrambler<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    state: Rc<ScramblerState<T>>,
}

impl<T> Scrambler<T>
where
    T: SimObject,
{
    pub fn new(parent: &Arc<Entity>, name: &str, spawner: Spawner, scramble: bool) -> Self {
        let entity = Arc::new(Entity::new(parent, name));

        Self {
            entity: entity.clone(),
            spawner: spawner.clone(),
            state: Rc::new(ScramblerState::new(entity, spawner, scramble)),
        }
    }

    pub fn connect_port_tx_a(&mut self, port_state: Rc<PortState<T>>) {
        if self.state.scramble {
            connect_tx!(self.state.buffer_b, connect_port_tx ; port_state);
        } else {
            connect_tx!(self.state.buffer_a, connect_port_tx ; port_state);
        }
    }

    pub fn connect_port_tx_b(&mut self, port_state: Rc<PortState<T>>) {
        if self.state.scramble {
            connect_tx!(self.state.buffer_a, connect_port_tx ; port_state);
        } else {
            connect_tx!(self.state.buffer_b, connect_port_tx ; port_state);
        }
    }

    pub fn port_rx_a(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.buffer_a, port_rx)
    }

    pub fn port_rx_b(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.buffer_b, port_rx)
    }

    pub async fn run(&self) -> SimResult {
        spawn_subcomponent!(self.spawner ; self.state.buffer_a);
        spawn_subcomponent!(self.spawner ; self.state.buffer_b);
        Ok(())
    }
}
