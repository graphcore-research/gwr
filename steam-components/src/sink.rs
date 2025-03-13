// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Sink components.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use steam_engine::port::{InPort, PortState};
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::enter;
use steam_track::entity::Entity;

use crate::{port_rx, take_option};

pub struct SinkState<T>
where
    T: SimObject,
{
    sunk_count: RefCell<usize>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> SinkState<T>
where
    T: SimObject,
{
    fn new(entity: &Arc<Entity>) -> Self {
        Self {
            sunk_count: RefCell::new(0),
            rx: RefCell::new(Some(InPort::new(entity, "rx"))),
        }
    }

    pub fn num_sunk(&self) -> usize {
        *self.sunk_count.borrow()
    }
}

#[derive(Clone, EntityDisplay)]
pub struct Sink<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    state: Rc<SinkState<T>>,
}

impl<T> Sink<T>
where
    T: SimObject,
{
    pub fn new(parent: &Arc<Entity>, name: &str) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        let state = Rc::new(SinkState::new(&entity));
        Self { entity, state }
    }

    pub fn port_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.rx, state)
    }

    pub async fn run(&self) -> SimResult {
        let rx = take_option!(self.state.rx);
        loop {
            let value = rx.get().await;
            enter!(self.entity ; value.tag());
            *self.state.sunk_count.borrow_mut() += 1;
        }
    }

    pub fn state(&self) -> Rc<SinkState<T>> {
        self.state.clone()
    }

    pub fn num_sunk(&self) -> usize {
        self.state.num_sunk()
    }
}
