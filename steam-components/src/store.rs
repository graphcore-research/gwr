// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A data store
//!
//! The [Store] is a component that can hold a number of items defined by its
//! capacity. The [Store] has two ports:
//!   - The `rx` port [InPort] which is used to put data into the store.
//!   - The `tx` port [OutPort] which is used to get data out of the store.
//!
//! A given [Store] is capable of holding any type as long as it implements the
//! [SimObject] trait. This trait has been implemented for a few builtin types
//! like `i32` and `usize` so that they can be used for simple testing.
//!
//! <br>
//!
//! # Build a store
//!
//! Here is an example creating a [Store]:
//!
//! ```rust
//! use std::rc::Rc;
//! use std::sync::Arc;
//!
//! use steam_components::store::Store;
//! use steam_engine::engine::Engine;
//! use steam_engine::executor::Spawner;
//! use steam_track::entity::Entity;
//!
//! fn build_store(engine: &Engine, parent: &Arc<Entity>, spawner: Spawner) -> Rc<Store<i32>> {
//!     // Create a store. It is passed:
//!     //   - a parent entity which provides its location within the simulation.
//!     //   - a name which should be unique within the parent.
//!     //   - a [spawner](steam_engine::executor::Spawner) that is used to spawn
//!     //     internal concurrent tasks.
//!     //   - a capacity.
//!     let store: Rc<Store<i32>> = Store::new_and_register(engine, parent, "store", spawner, 5);
//!     store
//! }
//! ```
//!
//! By default, the store enters a waiting state if the capacity is overflown,
//! but this behaviour can be changed to panic, by using the
//! `set_panic_on_overflow()` method.
//!
//! ```rust
//! use std::rc::Rc;
//! use std::sync::Arc;
//!
//! use steam_components::store::Store;
//! use steam_engine::engine::Engine;
//! use steam_engine::executor::Spawner;
//! use steam_track::entity::Entity;
//!
//! fn build_store_with_panic(
//!     engine: &Engine,
//!     parent: &Arc<Entity>,
//!     spawner: Spawner,
//! ) -> Rc<Store<i32>> {
//!     // Create a store that panics on overflow. Use `new_and_register()` as before,
//!     // then call `set_panic_on_overflow()` on the resulting struct.
//!     let store = Store::new_and_register(engine, parent, "store_panic", spawner, 5);
//!     store.set_panic_on_overflow();
//!     store
//! }
//! ```

//! # Connect a store
//!
//! Here is an example of a more complete simulation using a [Store] as well as
//! a [Source](crate::source::Source) to put data into the store and an
//! [Sink](crate::sink::Sink) to take the data out of the store.
//!
//! ```rust
//! use std::sync::Arc;
//! use steam_components::sink::Sink;
//! use steam_components::source::Source;
//! use steam_components::store::Store;
//! use steam_components::{connect_port, option_box_repeat};
//! use steam_engine::engine::Engine;
//! use steam_engine::run_simulation;
//!
//! // Every simulation is based around an `Engine`
//! let mut engine = Engine::default();
//!
//! // A spawner allows components to spawn activity
//! let spawner = engine.spawner();
//!
//! // Take a reference to the engine top to use as the parent
//! let top = engine.top();
//!
//! // Create the basic componets:
//! // The simplest use of the source is to inject the same value repeatedly.
//! let source = Source::new_and_register(&engine, top, "source", option_box_repeat!(1 ; 10));
//! // Create the store - its type will be derived from the connections to its ports.
//! let store = Store::new_and_register(&engine, top, "store", spawner, 5);
//! // Create the sink which will pull all of the data items out of the store
//! let sink = Sink::new_and_register(&engine, top, "sink");
//!
//! // Connect the ports together:
//! // The source will drive data into the store:
//! connect_port!(source, tx => store, rx);
//!
//! // The sink will pull data out of the store
//! connect_port!(store, tx => sink, rx);
//!
//! // The `run_simulation!` macro then spawns all active components
//! // and runs the simulation to completion.
//! run_simulation!(engine);
//! ```

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_engine::engine::Engine;
use steam_engine::events::repeated::Repeated;
use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortState};
use steam_engine::traits::{Event, Runnable, SimObject};
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::{enter, exit};

use crate::{connect_tx, port_rx, take_option};

/// The [`State`] of a [`Store`].
struct State<T>
where
    T: SimObject,
{
    entity: Arc<Entity>,
    capacity: usize,
    data: RefCell<VecDeque<T>>,
    panic_on_overflow: RefCell<bool>,
    level_change: Repeated<usize>,
}

impl<T> State<T>
where
    T: SimObject,
{
    /// Create a new store
    fn new(entity: &Arc<Entity>, capacity: usize) -> Self {
        Self {
            entity: entity.clone(),
            capacity,
            data: RefCell::new(VecDeque::with_capacity(capacity)),
            panic_on_overflow: RefCell::new(false),
            level_change: Repeated::new(usize::default()),
        }
    }

    /// Place an object into the store state.
    ///
    /// There must be room before this is called.
    fn push_value(&self, value: T) {
        enter!(self.entity ; value.tag());
        if *self.panic_on_overflow.borrow() {
            if self.data.borrow().len() >= self.capacity {
                panic!("Overflow in {:?}", self.entity.full_name());
            }
        } else {
            assert!(self.data.borrow().len() < self.capacity);
        }

        self.data.borrow_mut().push_back(value);
        self.level_change
            .notify_result(self.data.borrow().len())
            .unwrap();
    }

    /// Remove an object from the store state.
    ///
    /// There must be an object available to remove before this is called.
    fn pop_value(&self) -> T {
        let value = self.data.borrow_mut().pop_front().unwrap();
        self.level_change
            .notify_result(self.data.borrow().len())
            .unwrap();
        exit!(self.entity ; value.tag());
        value
    }
}

/// A component that can support a configurable number of objects.
///
/// Objects must support the [SimObject] trait.
#[derive(EntityDisplay)]
pub struct Store<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    state: Rc<State<T>>,

    tx: RefCell<Option<OutPort<T>>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> Store<T>
where
    T: SimObject,
{
    /// Basic store constructor
    ///
    /// **Panics** if `capacity` is 0.
    #[must_use]
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        spawner: Spawner,
        capacity: usize,
    ) -> Rc<Self> {
        assert_ne!(capacity, 0, "Unsupported Store with 0 capacity");
        let entity = Arc::new(Entity::new(parent, name));
        let state = Rc::new(State::new(&entity, capacity));
        let tx = OutPort::new(&entity, "tx");
        let rx = InPort::new(&entity, "rx");
        let rc_self = Rc::new(Self {
            entity,
            spawner,
            state,
            tx: RefCell::new(Some(tx)),
            rx: RefCell::new(Some(rx)),
        });
        engine.register(rc_self.clone());
        rc_self
    }

    pub fn connect_port_tx(&self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.tx, connect ; port_state);
    }

    #[must_use]
    pub fn port_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.rx, state)
    }

    #[must_use]
    pub fn fill_level(&self) -> usize {
        self.state.data.borrow().len()
    }

    pub fn set_panic_on_overflow(&self) {
        *self.state.panic_on_overflow.borrow_mut() = true;
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

async fn run_rx<T>(rx: InPort<T>, state: Rc<State<T>>) -> SimResult
where
    T: SimObject,
{
    let level_change = state.level_change.clone();
    let panic_on_overflow = *state.panic_on_overflow.borrow();
    loop {
        let level = state.data.borrow().len();
        if level < state.capacity || panic_on_overflow {
            let value = rx.get().await;
            state.push_value(value);
        } else {
            level_change.listen().await;
        }
    }
}

async fn run_tx<T>(tx: OutPort<T>, state: Rc<State<T>>) -> SimResult
where
    T: SimObject,
{
    let level_change = state.level_change.clone();
    loop {
        let level = state.data.borrow().len();
        if level > 0 {
            // Wait for something to actually want the store value
            tx.try_put().await?;
            let value = state.pop_value();
            tx.put(value).await?;
        } else {
            level_change.listen().await;
        }
    }
}
