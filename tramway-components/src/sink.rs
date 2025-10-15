// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A data sink.
//!
//! A [Sink] is an object that will accept and count all the data that
//! is received on its input port.
//!
//! # Ports
//!
//! This component has:
//!  - One [input port](tramway_engine::port::InPort): `rx`

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use tramway_engine::engine::Engine;
use tramway_engine::port::{InPort, PortStateResult};
use tramway_engine::traits::{Runnable, SimObject};
use tramway_engine::types::{SimError, SimResult};
use tramway_model_builder::EntityDisplay;
use tramway_track::enter;
use tramway_track::entity::Entity;

use crate::{port_rx, take_option};

#[derive(EntityDisplay)]
pub struct Sink<T>
where
    T: SimObject,
{
    pub entity: Rc<Entity>,
    sunk_count: RefCell<usize>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> Sink<T>
where
    T: SimObject,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Rc<Entity>,
        name: &str,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let rx = InPort::new(&entity, "rx");
        let rc_self = Rc::new(Self {
            entity,
            sunk_count: RefCell::new(0),
            rx: RefCell::new(Some(rx)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        port_rx!(self.rx, state)
    }

    #[must_use]
    pub fn num_sunk(&self) -> usize {
        *self.sunk_count.borrow()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Sink<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        loop {
            let value = rx.get()?.await;
            enter!(self.entity ; value.id());
            *self.sunk_count.borrow_mut() += 1;
        }
    }
}
