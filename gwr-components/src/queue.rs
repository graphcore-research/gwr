// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A generic queue.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::rc::Rc;

use gwr_engine::events::repeated::Repeated;
use gwr_engine::sim_error;
use gwr_engine::traits::{Event, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::EntityDisplay;
use gwr_track::entity::Entity;
use gwr_track::{enter, exit};

/// A generic queue for simulation objects.
#[derive(EntityDisplay)]
pub struct Queue<T>
where
    T: SimObject,
{
    entity: Rc<Entity>,
    capacity: Option<usize>,
    data: RefCell<VecDeque<T>>,
    queue_changed: Repeated<()>,
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
    /// Create a new queue.
    ///
    /// Returns a [`SimError`] if `capacity` is `Some(0)`.
    pub fn new(parent: &Rc<Entity>, name: &str, capacity: Option<usize>) -> Result<Self, SimError> {
        if capacity == Some(0) {
            return sim_error!("Unsupported Queue with 0 capacity");
        }

        Ok(Self {
            entity: Rc::new(Entity::new(parent, name)),
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
        enter!(self.entity ; value.id());
        self.data.borrow_mut().push_back(value);
        self.queue_changed.notify();
    }

    /// Pop the oldest value from the queue.
    #[must_use]
    pub fn pop_front(&self) -> Option<T> {
        let value = self.data.borrow_mut().pop_front();
        if let Some(ref value) = value {
            exit!(self.entity ; value.id());
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

        exit!(self.entity ; value.id());
        self.queue_changed.notify();
        Some(value)
    }
}
