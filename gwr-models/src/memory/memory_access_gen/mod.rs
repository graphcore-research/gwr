// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A data source.
//!
//! The data source produces data as defined by the [DataGenerator] that is
//! provided.
//!
//! # Ports
//!
//! This component has one port:
//!  - One [output port](gwr_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::types::DataGenerator;
use gwr_components::{connect_tx, port_rx, take_option};
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, Runnable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::Id;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;

use crate::memory::traits::AccessMemory;

pub mod random;
pub mod strided;

#[derive(EntityGet, EntityDisplay)]
pub struct MemoryAccessGen<T>
where
    T: SimObject + AccessMemory,
{
    entity: Rc<Entity>,
    spawner: Spawner,
    data_generator: RefCell<Option<DataGenerator<T>>>,
    rx: RefCell<Option<InPort<T>>>,
    tx: RefCell<Option<OutPort<T>>>,
    payload_bytes_received: Rc<RefCell<usize>>,
    max_outstanding_requests: RefCell<Option<usize>>,
}

impl<T> MemoryAccessGen<T>
where
    T: SimObject + AccessMemory,
{
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        data_generator: DataGenerator<T>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);
        let tx = OutPort::new_with_renames(&entity, "tx", aka);
        let rc_self = Rc::new(Self {
            entity,
            spawner: engine.spawner(),
            data_generator: RefCell::new(Some(data_generator)),
            rx: RefCell::new(Some(rx)),
            tx: RefCell::new(Some(tx)),
            payload_bytes_received: Rc::new(RefCell::new(0)),
            max_outstanding_requests: RefCell::new(None),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        data_generator: DataGenerator<T>,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, data_generator)
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

    pub fn set_max_outstanding_requests(
        &self,
        max_outstanding_requests: Option<usize>,
    ) -> SimResult {
        if let Some(outstanding_requests) = max_outstanding_requests
            && outstanding_requests == 0
        {
            return sim_error!("Invalid max_outstanding_requests value of 0. It will deadlock");
        }
        *self.max_outstanding_requests.borrow_mut() = max_outstanding_requests;
        Ok(())
    }

    async fn enforce_max_outstanding_limit(
        &self,
        expected: &Rc<RefCell<HashSet<Id>>>,
        outstanding_available: &Rc<Repeated<()>>,
    ) {
        let Some(max_outstanding_requests) = *self.max_outstanding_requests.borrow() else {
            return;
        };
        loop {
            if expected.borrow().len() < max_outstanding_requests {
                break;
            }
            outstanding_available.listen().await;
        }
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
        let outstanding_available = Rc::new(Repeated::new(()));
        let rx = take_option!(self.rx);
        let mut tx = take_option!(self.tx);

        {
            let expected = expected.clone();
            let payload_bytes_received = self.payload_bytes_received.clone();
            let outstanding_available = outstanding_available.clone();
            self.spawner.spawn(async move {
                run_input(rx, expected, payload_bytes_received, outstanding_available).await?;
                Ok(())
            });
        }

        for value in data_generator {
            self.enforce_max_outstanding_limit(&expected, &outstanding_available)
                .await;
            let id = value.id();
            if !expected.borrow_mut().insert(id) {
                return sim_error!("Generator produced duplicate ID {id}");
            }
            tx.put(value)?.await;
        }

        Ok(())
    }
}

async fn run_input<T>(
    mut rx: InPort<T>,
    expected: Rc<RefCell<HashSet<Id>>>,
    payload_bytes_received: Rc<RefCell<usize>>,
    outstanding_available: Rc<Repeated<()>>,
) -> SimResult
where
    T: SimObject + AccessMemory,
{
    loop {
        let received = rx.get()?.await;
        let received_id = received.id();
        if !expected.borrow_mut().remove(&received_id) {
            return sim_error!("{received_id} received when not expected");
        }
        *payload_bytes_received.borrow_mut() += received.access_size_bytes();
        outstanding_available.notify();
    }
}
