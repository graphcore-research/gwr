// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Generic queues.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
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

/// A generic queue for simulation objects, without ports or runnable behavior.
#[derive(EntityGet, EntityDisplay)]
pub struct QueueCore<T>
where
    T: SimObject,
{
    entity: Rc<Entity>,
    capacity: Option<usize>,
    data: RefCell<VecDeque<T>>,
    queue_changed: Repeated<()>,
}

impl<T> fmt::Debug for QueueCore<T>
where
    T: SimObject,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueCore")
            .field("entity", &self.entity)
            .finish()
    }
}

impl<T> QueueCore<T>
where
    T: SimObject,
{
    /// Create a new queue.
    ///
    /// Returns a [`SimError`] if `capacity` is `Some(0)`.
    pub fn new(parent: &Rc<Entity>, name: &str, capacity: Option<usize>) -> Result<Self, SimError> {
        if capacity == Some(0) {
            return sim_error!("Unsupported Queue with 0 capacity");
        }

        let entity = Rc::new(Entity::new(parent, name));
        if let Some(capacity) = capacity {
            entity.track_capacity(capacity, "objects");
        }

        Ok(Self {
            entity,
            capacity,
            data: RefCell::new(VecDeque::new()),
            queue_changed: Repeated::default(),
        })
    }

    /// Return the current queue length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.borrow().len()
    }

    /// Return whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.borrow().is_empty()
    }

    /// Return whether the queue is full.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.capacity.is_some_and(|capacity| self.len() >= capacity)
    }

    /// Return a snapshot of the queue contents by copying all values.
    #[must_use]
    pub fn values(&self) -> Vec<T> {
        self.data.borrow().iter().cloned().collect()
    }

    /// Return an event that fires whenever the queue contents change.
    #[must_use]
    pub fn changed_event(&self) -> Repeated<()> {
        self.queue_changed.clone()
    }

    /// Push a value into the queue, waiting until there is space if required.
    pub async fn push(&self, value: T) -> SimResult {
        let mut value = Some(value);
        loop {
            if !self.is_full() {
                self.push_now(value.take().expect("value should still be present"));
                return Ok(());
            }

            self.queue_changed.listen().await;
        }
    }

    fn push_now(&self, value: T) {
        self.entity.track_enter(value.id());
        self.data.borrow_mut().push_back(value);
        self.queue_changed.notify();
    }

    /// Pop the oldest value from the queue.
    #[must_use]
    pub fn pop_front(&self) -> Option<T> {
        let value = self.data.borrow_mut().pop_front();
        if let Some(ref value) = value {
            self.entity.track_exit(value.id());
            self.queue_changed.notify();
        }
        value
    }

    /// Remove the first value matching the predicate.
    #[must_use]
    pub fn remove_where<F>(&self, predicate: F) -> Option<T>
    where
        F: FnMut(&T) -> bool,
    {
        let mut queue = self.data.borrow_mut();
        let index = queue.iter().position(predicate)?;
        let value = queue.remove(index)?;
        drop(queue);

        self.entity.track_exit(value.id());
        self.queue_changed.notify();
        Some(value)
    }
}

/// A generic queue component with `rx` and `tx` ports.
#[derive(EntityGet, EntityDisplay)]
pub struct Queue<T>
where
    T: SimObject,
{
    entity: Rc<Entity>,
    spawner: Spawner,
    queue: Rc<QueueCore<T>>,
    rx: RefCell<Option<InPort<T>>>,
    tx: RefCell<Option<OutPort<T>>>,
}

impl<T> fmt::Debug for Queue<T>
where
    T: SimObject,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queue")
            .field("entity", &self.entity)
            .finish()
    }
}

impl<T> Queue<T>
where
    T: SimObject,
{
    /// Create and register a new queue component.
    ///
    /// Returns a [`SimError`] if `capacity` is `Some(0)`.
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        capacity: Option<usize>,
    ) -> Result<Rc<Self>, SimError> {
        let spawner = engine.spawner();
        let queue = QueueCore::new(parent, name, capacity)?;
        let entity = queue.entity.clone();
        let tx = OutPort::new_with_renames(&entity, "tx", aka);
        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);
        let rc_self = Rc::new(Self {
            entity,
            spawner,
            queue: Rc::new(queue),
            rx: RefCell::new(Some(rx)),
            tx: RefCell::new(Some(tx)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    /// Create and register a new queue component.
    ///
    /// Returns a [`SimError`] if `capacity` is `Some(0)`.
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        capacity: Option<usize>,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, capacity)
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.tx, connect ; port_state)
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        port_rx!(self.rx, state)
    }

    /// Return the current queue length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Return whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Return whether the queue is full.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.queue.is_full()
    }

    /// Return a snapshot of the queue contents by copying all values.
    #[must_use]
    pub fn values(&self) -> Vec<T> {
        self.queue.values()
    }

    /// Return an event that fires whenever the queue contents change.
    #[must_use]
    pub fn changed_event(&self) -> Repeated<()> {
        self.queue.changed_event()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Queue<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        let queue = self.queue.clone();
        self.spawner.spawn(async move { run_rx(rx, queue).await });

        let tx = take_option!(self.tx);
        let queue = self.queue.clone();
        self.spawner.spawn(async move { run_tx(tx, queue).await });
        Ok(())
    }
}

async fn run_rx<T>(rx: InPort<T>, queue: Rc<QueueCore<T>>) -> SimResult
where
    T: SimObject,
{
    let queue_changed = queue.changed_event();
    loop {
        if queue.is_full() {
            queue_changed.listen().await;
        } else {
            let value = rx.get()?.await;
            queue.push(value).await?;
        }
    }
}

async fn run_tx<T>(tx: OutPort<T>, queue: Rc<QueueCore<T>>) -> SimResult
where
    T: SimObject,
{
    let queue_changed = queue.changed_event();
    loop {
        if queue.is_empty() {
            queue_changed.listen().await;
        } else {
            tx.try_put()?.await;
            if let Some(value) = queue.pop_front() {
                tx.put(value)?.await;
            }
        }
    }
}
