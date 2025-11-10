// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A data source.
//!
//! The data source produces data as defined by the [DataGenerator] that is
//! provided.
//!
//! # Ports
//!
//! This component has:
//!  - One [output port](gwr_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::port::{OutPort, PortStateResult};
use gwr_engine::traits::{Runnable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::entity::Entity;
use gwr_track::exit;
use gwr_track::tracker::aka::Aka;

#[macro_export]
macro_rules! option_box_repeat {
    ($value:expr ; $repeat:expr) => {
        Some(Box::new(std::iter::repeat($value).take($repeat)))
    };
}
use crate::types::DataGenerator;
use crate::{connect_tx, take_option};

#[macro_export]
macro_rules! option_box_chain {
    ($value1:expr , $value2:expr) => {
        Some(Box::new((*($value1.unwrap())).chain(*($value2.unwrap()))))
    };
}

#[derive(EntityGet, EntityDisplay)]
pub struct Source<T>
where
    T: SimObject,
{
    entity: Rc<Entity>,
    data_generator: RefCell<Option<DataGenerator<T>>>,
    tx: RefCell<Option<OutPort<T>>>,
}

impl<T> Source<T>
where
    T: SimObject,
{
    pub fn new_and_register_with_renames(
        engine: &Engine,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        data_generator: Option<DataGenerator<T>>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let tx = OutPort::new_with_renames(&entity, "tx", aka);
        let rc_self = Rc::new(Self {
            entity,
            data_generator: RefCell::new(data_generator),
            tx: RefCell::new(Some(tx)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn new_and_register(
        engine: &Engine,
        parent: &Rc<Entity>,
        name: &str,
        data_generator: Option<DataGenerator<T>>,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, parent, name, None, data_generator)
    }

    #[must_use]
    pub fn entity(&self) -> &Rc<Entity> {
        &self.entity
    }

    pub fn set_generator(&self, data_generator: Option<DataGenerator<T>>) {
        *self.data_generator.borrow_mut() = data_generator;
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.tx, connect ; port_state)
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Source<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        let mut data_generator = match self.data_generator.borrow_mut().take() {
            Some(data_generator) => data_generator,
            None => return Ok(()),
        };

        let tx = take_option!(self.tx);
        loop {
            let value = data_generator.next();
            if let Some(value) = value {
                exit!(self.entity ; value.id());
                tx.put(value)?.await;
            } else {
                break;
            }
        }
        Ok(())
    }
}
