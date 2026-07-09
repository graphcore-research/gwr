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

use crate::{connect_tx, port_rx, take_option};

mod byte_store;
mod object_store;

pub use byte_store::ByteStore;
pub use object_store::ObjectStore;

type ObjectToCapacity<T> = fn(&T) -> usize;

struct State<T>
where
    T: SimObject,
{
    entity: Rc<Entity>,
    capacity: usize,
    capacity_unit: RefCell<String>,
    used: RefCell<usize>,
    data: RefCell<VecDeque<T>>,
    error_on_overflow: RefCell<bool>,
    level_change: Repeated<usize>,
    object_to_capacity: ObjectToCapacity<T>,
}

impl<T> State<T>
where
    T: SimObject,
{
    fn new(entity: &Rc<Entity>, capacity: usize, object_to_capacity: ObjectToCapacity<T>) -> Self {
        Self {
            entity: entity.clone(),
            capacity,
            capacity_unit: RefCell::new("objects".to_string()),
            used: RefCell::new(0),
            data: RefCell::new(VecDeque::new()),
            error_on_overflow: RefCell::new(false),
            level_change: Repeated::new(usize::default()),
            object_to_capacity,
        }
    }

    fn has_capacity_for(&self, units: usize) -> bool {
        units <= self.capacity - *self.used.borrow()
    }

    fn check_units_can_fit(&self, units: usize) -> SimResult {
        if units > self.capacity {
            let capacity_unit = self.capacity_unit.borrow();
            return sim_error!(
                "Cannot store {units} {capacity_unit} in {:?} with capacity {}",
                self.entity.full_name(),
                self.capacity
            );
        }
        Ok(())
    }

    fn push_value(&self, value: T) -> SimResult {
        let units = (self.object_to_capacity)(&value);
        self.entity.track_enter(value.id());
        if *self.error_on_overflow.borrow() {
            if !self.has_capacity_for(units) {
                return sim_error!("Overflow in {:?}", self.entity.full_name());
            }
        } else {
            assert!(self.has_capacity_for(units));
        }

        self.data.borrow_mut().push_back(value);
        *self.used.borrow_mut() += units;
        self.level_change.notify_result(*self.used.borrow());
        Ok(())
    }

    fn pop_value(&self) -> Result<T, SimError> {
        let value = self.data.borrow_mut().pop_front().unwrap();
        *self.used.borrow_mut() -= (self.object_to_capacity)(&value);
        self.level_change.notify_result(*self.used.borrow());
        self.entity.track_exit(value.id());
        Ok(value)
    }
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
    state: Rc<State<T>>,
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
        object_to_capacity: ObjectToCapacity<T>,
    ) -> Result<Self, SimError> {
        if capacity == 0 {
            return sim_error!("Unsupported Store with capacity of 0");
        }
        Ok(Self {
            entity: entity.clone(),
            spawner: engine.spawner(),
            state: Rc::new(State::new(entity, capacity, object_to_capacity)),
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
        *self.state.used.borrow()
    }

    pub fn set_error_on_overflow(&self) {
        *self.state.error_on_overflow.borrow_mut() = true;
    }

    pub fn set_capacity_unit(&self, capacity_unit: impl Into<String>) {
        *self.state.capacity_unit.borrow_mut() = capacity_unit.into();
    }

    #[must_use]
    pub fn get_level_change_event(&self) -> Repeated<usize> {
        self.state.level_change.clone()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Store<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        let state = self.state.clone();
        self.spawner.spawn(async move { run_rx(rx, state).await });

        let tx = take_option!(self.tx);
        let state = self.state.clone();
        self.spawner.spawn(async move { run_tx(tx, state).await });
        Ok(())
    }
}

async fn run_rx<T>(mut rx: InPort<T>, state: Rc<State<T>>) -> SimResult
where
    T: SimObject,
{
    let level_change = state.level_change.clone();
    loop {
        let value = rx.start_get()?.await;
        let units = (state.object_to_capacity)(&value);
        state.check_units_can_fit(units)?;
        while !state.has_capacity_for(units) && !*state.error_on_overflow.borrow() {
            level_change.listen().await;
        }
        state.push_value(value)?;
        rx.finish_get();
    }
}

async fn run_tx<T>(mut tx: OutPort<T>, state: Rc<State<T>>) -> SimResult
where
    T: SimObject,
{
    let level_change = state.level_change.clone();
    loop {
        let level = state.data.borrow().len();
        if level > 0 {
            tx.try_put()?.await;
            let value = state.pop_value()?;
            tx.put(value)?.await;
        } else {
            level_change.listen().await;
        }
    }
}
