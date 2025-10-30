// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! An example component which can switch the how its inputs are connected.
//!
//! For latest usage run:
//! ```bash
//! cargo run --bin scrambler -- --help
//! ```
//!
//! # Examples
//!
//! Get the two inputs in the same order:
//! ```bash
//! $ cargo run --bin scrambler
//! Input order: 1, 2
//! ```
//!
//! Switch the two inputs:
//! ```bash
//! $ cargo run --bin scrambler -- -s
//! Input order: 2, 1
//! ```
//!
//! # Ports
//!
//! This component has four ports
//!  - Two [input port](gwr_engine::port::InPort): `rx_a`, `rx_b`
//!  - Two [output port](gwr_engine::port::OutPort): `tx_a`, `tx_b`

use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::store::Store;
use gwr_engine::engine::Engine;
use gwr_engine::port::PortStateResult;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::SimObject;
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, Runnable};
use gwr_track::entity::Entity;

#[derive(EntityDisplay, Runnable)]
pub struct Scrambler<T>
where
    T: SimObject,
{
    pub entity: Rc<Entity>,
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
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        scramble: bool,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let buffer_a = Store::new_and_register(engine, clock, &entity, "buffer_a", 1)?;
        let buffer_b = Store::new_and_register(engine, clock, &entity, "buffer_b", 1)?;

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
