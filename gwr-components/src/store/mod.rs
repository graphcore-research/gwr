// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A data store.
//!
//! [ObjectStore] builds an object-counted [Store], while [ByteStore] builds a
//! byte-counted [Store] using
//! [`total_bytes`](gwr_engine::traits::TotalBytes::total_bytes). The returned
//! [Store] is the registered component in both cases.
//!
//! # Ports
//!
//! This component has the following ports:
//!   - The `rx` port [InPort] which is used to put data into the store.
//!   - The `tx` port [OutPort] which is used to get data out of the store.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Event, Runnable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;

use crate::capacity_allocator::CapacityAllocator;
use crate::{connect_tx, port_rx, take_option};

mod byte_store;
mod object_store;

pub use byte_store::ByteStore;
pub use object_store::ObjectStore;

type ObjectToCapacity<T> = fn(&T) -> usize;

#[derive(Clone)]
struct State<T>
where
    T: SimObject,
{
    entity: Rc<Entity>,
    capacity: CapacityAllocator,
    data: Rc<RefCell<VecDeque<T>>>,
    error_on_overflow: Rc<RefCell<bool>>,
    object_to_capacity: ObjectToCapacity<T>,
}

/// A component that can support a configurable number of capacity units.
///
/// Use [ObjectStore] to build an object-counted store and [ByteStore] to build
/// a byte-counted store.
#[derive(EntityGet, EntityDisplay)]
pub struct Store<T>
where
    T: SimObject,
{
    entity: Rc<Entity>,
    spawner: Spawner,
    capacity: CapacityAllocator,
    data: Rc<RefCell<VecDeque<T>>>,
    error_on_overflow: Rc<RefCell<bool>>,
    object_to_capacity: ObjectToCapacity<T>,
    tx: RefCell<Option<OutPort<T>>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> Store<T>
where
    T: SimObject,
{
    fn new(
        engine: &Engine,
        clock: &Clock,
        entity: &Rc<Entity>,
        aka: Option<&Aka>,
        capacity: usize,
        capacity_unit: &str,
        object_to_capacity: ObjectToCapacity<T>,
    ) -> Result<Self, SimError> {
        if capacity == 0 {
            return sim_error!("Unsupported Store with capacity of 0");
        }
        let capacity = CapacityAllocator::for_entity(entity, capacity, capacity_unit)?;
        Ok(Self {
            entity: entity.clone(),
            spawner: engine.spawner(),
            capacity,
            data: Rc::new(RefCell::new(VecDeque::new())),
            error_on_overflow: Rc::new(RefCell::new(false)),
            object_to_capacity,
            tx: RefCell::new(Some(OutPort::new_with_renames(entity, "tx", aka))),
            rx: RefCell::new(Some(InPort::new_with_renames(
                engine, clock, entity, "rx", aka,
            ))),
        })
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.tx, connect ; port_state)
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        port_rx!(self.rx, state)
    }

    #[must_use]
    pub fn capacity_used(&self) -> usize {
        self.capacity.used()
    }

    pub fn set_error_on_overflow(&self) {
        *self.error_on_overflow.borrow_mut() = true;
    }

    #[must_use]
    pub fn get_level_change_event(&self) -> Repeated<usize> {
        self.capacity.level_change_event()
    }

    fn state(&self) -> State<T> {
        State {
            entity: self.entity.clone(),
            capacity: self.capacity.clone(),
            data: self.data.clone(),
            error_on_overflow: self.error_on_overflow.clone(),
            object_to_capacity: self.object_to_capacity,
        }
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Store<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        let state = self.state();
        self.spawner.spawn(async move { state.run_rx(rx).await });

        let tx = take_option!(self.tx);
        let state = self.state();
        self.spawner.spawn(async move { state.run_tx(tx).await });
        Ok(())
    }
}

impl<T> State<T>
where
    T: SimObject,
{
    fn check_units_can_fit(&self, units: usize) -> SimResult {
        if units > self.capacity.capacity() {
            return sim_error!(
                "Cannot store {units} {} in {:?} with capacity {}",
                self.capacity.capacity_unit(),
                self.entity.full_name(),
                self.capacity.capacity()
            );
        }
        Ok(())
    }

    fn push_value(&self, value: T) -> SimResult {
        let units = (self.object_to_capacity)(&value);
        self.capacity.allocate(units)?;
        self.entity.track_enter(value.id());
        self.data.borrow_mut().push_back(value);
        Ok(())
    }

    fn pop_value(&self) -> Result<T, SimError> {
        let value = self.data.borrow_mut().pop_front().unwrap();
        self.capacity.release((self.object_to_capacity)(&value));
        self.entity.track_exit(value.id());
        Ok(value)
    }

    async fn run_rx(&self, mut rx: InPort<T>) -> SimResult {
        loop {
            let value = rx.start_get()?.await;
            let units = (self.object_to_capacity)(&value);
            self.check_units_can_fit(units)?;
            if !*self.error_on_overflow.borrow() {
                self.capacity.wait_for_capacity(units).await?;
            }
            self.push_value(value)?;
            rx.finish_get();
        }
    }

    async fn run_tx(&self, mut tx: OutPort<T>) -> SimResult {
        let level_change = self.capacity.level_change_event();
        loop {
            let level = self.data.borrow().len();
            if level > 0 {
                tx.try_put()?.await;
                let value = self.pop_value()?;
                tx.put(value)?.await;
            } else {
                level_change.listen().await;
            }
        }
    }
}
