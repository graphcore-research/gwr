// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A data source.
//!
//! The data source produces data as defined by the [DataGenerator] that is
//! provided.
//!
//! # Ports
//!
//! This component has:
//!  - One [output port](tramway_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use tramway_engine::engine::Engine;
use tramway_engine::port::{OutPort, PortStateResult};
use tramway_engine::traits::{Runnable, SimObject};
use tramway_engine::types::{SimError, SimResult};
use tramway_model_builder::EntityDisplay;
use tramway_track::entity::Entity;
use tramway_track::exit;

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

#[derive(EntityDisplay)]
pub struct Source<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    data_generator: RefCell<Option<DataGenerator<T>>>,
    tx: RefCell<Option<OutPort<T>>>,
}

impl<T> Source<T>
where
    T: SimObject,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        data_generator: Option<DataGenerator<T>>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));
        let tx = OutPort::new(&entity, "tx");
        let rc_self = Rc::new(Self {
            entity,
            data_generator: RefCell::new(data_generator),
            tx: RefCell::new(Some(tx)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    #[must_use]
    pub fn entity(&self) -> &Arc<Entity> {
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
