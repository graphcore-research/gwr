// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A data source.
//!
//! The data source produces data as defined by the [DataGenerator] that is
//! provided.
//!
//! # Ports
//!
//! This component has one port:
//!  - One [output port](steam_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use steam_engine::port::{OutPort, PortState};
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::exit;

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

struct SourceState<T>
where
    T: SimObject,
{
    data_generator: RefCell<Option<DataGenerator<T>>>,
    tx: RefCell<Option<OutPort<T>>>,
}

impl<T> SourceState<T>
where
    T: SimObject,
{
    fn new(entity: Arc<Entity>, data_generator: Option<DataGenerator<T>>) -> Self {
        Self {
            data_generator: RefCell::new(data_generator),
            tx: RefCell::new(Some(OutPort::new(entity, "tx"))),
        }
    }
}

#[derive(Clone, EntityDisplay)]
pub struct Source<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    state: Rc<SourceState<T>>,
}

impl<T> Source<T>
where
    T: SimObject,
{
    pub fn new(parent: &Arc<Entity>, name: &str, data_generator: Option<DataGenerator<T>>) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        Self {
            entity: entity.clone(),
            state: Rc::new(SourceState::new(entity, data_generator)),
        }
    }

    pub fn entity(&self) -> &Arc<Entity> {
        &self.entity
    }

    pub fn set_generator(&self, data_generator: Option<DataGenerator<T>>) {
        *self.state.data_generator.borrow_mut() = data_generator;
    }

    pub fn connect_port_tx(&self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.tx, connect ; port_state);
    }

    pub async fn run(&self) -> SimResult {
        let mut data_generator = match self.state.data_generator.borrow_mut().take() {
            Some(data_generator) => data_generator,
            None => return Ok(()),
        };

        let tx = take_option!(self.state.tx);
        loop {
            let value = data_generator.next();
            if let Some(value) = value {
                exit!(self.entity ; value.tag());
                tx.put(value).await?;
            } else {
                break;
            }
        }
        Ok(())
    }
}
