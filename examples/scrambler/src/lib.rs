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

use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_components::store::Store;
use steam_engine::engine::Engine;
use steam_engine::executor::Spawner;
use steam_engine::port::PortStateResult;
use steam_engine::traits::SimObject;
use steam_engine::types::{SimError, SimResult};
use steam_model_builder::{EntityDisplay, Runnable};
use steam_track::entity::Entity;

#[derive(EntityDisplay, Runnable)]
pub struct Scrambler<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    scramble: bool,
    buffer_a: Rc<Store<T>>,
    buffer_b: Rc<Store<T>>,
}

impl<T> Scrambler<T>
where
    T: SimObject,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        spawner: Spawner,
        scramble: bool,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));
        let buffer_a = Store::new_and_register(engine, &entity, "buffer_a", spawner.clone(), 1)?;
        let buffer_b = Store::new_and_register(engine, &entity, "buffer_b", spawner, 1)?;

        let rc_self = Rc::new(Self {
            entity: entity.clone(),
            scramble,
            buffer_a,
            buffer_b,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn connect_port_tx_a(&self, port_state: PortStateResult<T>) -> SimResult {
        if self.scramble {
            self.buffer_b.connect_port_tx(port_state)
        } else {
            self.buffer_a.connect_port_tx(port_state)
        }
    }

    pub fn connect_port_tx_b(&self, port_state: PortStateResult<T>) -> SimResult {
        if self.scramble {
            self.buffer_a.connect_port_tx(port_state)
        } else {
            self.buffer_b.connect_port_tx(port_state)
        }
    }

    pub fn port_rx_a(&self) -> PortStateResult<T> {
        self.buffer_a.port_rx()
    }

    pub fn port_rx_b(&self) -> PortStateResult<T> {
        self.buffer_b.port_rx()
    }
}
