// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A data source.
//!
//! The data source produces data as defined by the [DataGenerator] that is
//! provided.
//!
//! # Ports
//!
//! This component has one port:
//!  - One [output port](tramway_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use tramway_components::types::DataGenerator;
use tramway_components::{connect_tx, port_rx, take_option};
use tramway_engine::engine::Engine;
use tramway_engine::executor::Spawner;
use tramway_engine::port::{InPort, OutPort, PortStateResult};
use tramway_engine::sim_error;
use tramway_engine::traits::{Runnable, SimObject};
use tramway_engine::types::{SimError, SimResult};
use tramway_model_builder::EntityDisplay;
use tramway_track::Id;
use tramway_track::entity::Entity;

use crate::memory::traits::AccessMemory;

pub mod random;
pub mod strided;

#[derive(EntityDisplay)]
pub struct MemoryAccessGen<T>
where
    T: SimObject + AccessMemory,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    data_generator: RefCell<Option<DataGenerator<T>>>,
    rx: RefCell<Option<InPort<T>>>,
    tx: RefCell<Option<OutPort<T>>>,
    payload_bytes_received: Rc<RefCell<usize>>,
}

impl<T> MemoryAccessGen<T>
where
    T: SimObject + AccessMemory,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        data_generator: DataGenerator<T>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));
        let rx = InPort::new(&entity, "rx");
        let tx = OutPort::new(&entity, "tx");
        let rc_self = Rc::new(Self {
            entity,
            spawner: engine.spawner(),
            data_generator: RefCell::new(Some(data_generator)),
            rx: RefCell::new(Some(rx)),
            tx: RefCell::new(Some(tx)),
            payload_bytes_received: Rc::new(RefCell::new(0)),
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

    pub fn port_rx(&self) -> PortStateResult<T> {
        port_rx!(self.rx, state)
    }

    pub fn payload_bytes_received(&self) -> usize {
        *self.payload_bytes_received.borrow()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for MemoryAccessGen<T>
where
    T: SimObject + AccessMemory,
{
    async fn run(&self) -> SimResult {
        let data_generator = match self.data_generator.borrow_mut().take() {
            Some(data_generator) => data_generator,
            None => return Ok(()),
        };

        // Use a HashSet so that memory accesses are permitted in any order
        let expected = Rc::new(RefCell::new(HashSet::new()));
        let rx = take_option!(self.rx);
        let tx = take_option!(self.tx);

        {
            let expected = expected.clone();
            let payload_bytes_received = self.payload_bytes_received.clone();
            self.spawner.spawn(async move {
                run_input(rx, expected, payload_bytes_received).await?;
                Ok(())
            });
        }

        for value in data_generator {
            let id = value.id();
            if !expected.borrow_mut().insert(id) {
                return sim_error!(format!("Generator produced duplicate ID {id}"));
            }
            tx.put(value)?.await;
        }

        Ok(())
    }
}

async fn run_input<T>(
    rx: InPort<T>,
    expected: Rc<RefCell<HashSet<Id>>>,
    payload_bytes_received: Rc<RefCell<usize>>,
) -> SimResult
where
    T: SimObject + AccessMemory,
{
    loop {
        let received = rx.get()?.await;
        let received_id = received.id();
        if !expected.borrow_mut().remove(&received_id) {
            return sim_error!(format!("{received_id} received when not expected"));
        }
        *payload_bytes_received.borrow_mut() += received.access_size_bytes();
    }
}
