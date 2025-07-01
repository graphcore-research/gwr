// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Sink components.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_engine::engine::Engine;
use steam_engine::port::{InPort, PortState};
use steam_engine::traits::{Runnable, SimObject};
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::enter;
use steam_track::entity::Entity;

use crate::{port_rx, take_option};

#[derive(EntityDisplay)]
pub struct Sink<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    sunk_count: RefCell<usize>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> Sink<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new_and_register(engine: &Engine, parent: &Arc<Entity>, name: &str) -> Rc<Self> {
        let entity = Arc::new(Entity::new(parent, name));
        let rx = InPort::new(&entity, "rx");
        let rc_self = Rc::new(Self {
            entity,
            sunk_count: RefCell::new(0),
            rx: RefCell::new(Some(rx)),
        });
        engine.register(rc_self.clone());
        rc_self
    }

    #[must_use]
    pub fn port_rx(&self) -> Rc<PortState<T>> {
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
            let value = rx.get().await;
            enter!(self.entity ; value.tag());
            *self.sunk_count.borrow_mut() += 1;
        }
    }
}
