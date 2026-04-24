// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! An event that can be triggered multiple times. The event allows
//! the notifier to pass a custom result to its listeners on each
//! notification, using the `notify_result()` method. Alternatively,
//! the last set result will be provided to the listeners. If no
//! result has been set, the default value for the result type will
//! be used.

use std::cell::{Cell, RefCell};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use futures::Future;
use futures::future::FusedFuture;

use super::waiting::Waiting;
use crate::traits::{BoxFuture, Event};

pub struct RepeatedState<T>
where
    T: Copy,
{
    waiting: Waiting,
    generation: Cell<u64>,
    result: RefCell<T>,
}

impl<T> RepeatedState<T>
where
    T: Copy,
{
    pub fn new(value: T) -> Self {
        Self {
            waiting: Waiting::new(),
            generation: Cell::new(0),
            result: RefCell::new(value),
        }
    }
}

impl Default for RepeatedState<()> {
    fn default() -> Self {
        Self::new(())
    }
}

#[derive(Clone)]
pub struct Repeated<T>
where
    T: Copy,
{
    state: Rc<RepeatedState<T>>,
}

pub struct RepeatedFuture<T>
where
    T: Copy,
{
    state: Rc<RepeatedState<T>>,
    done: bool,
    listener_id: Option<u64>,
    observed_generation: u64,
}

impl<T> FusedFuture for RepeatedFuture<T>
where
    T: Copy,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

impl<T> Repeated<T>
where
    T: Copy,
{
    pub fn with_value(value: T) -> Self {
        Self {
            state: Rc::new(RepeatedState::new(value)),
        }
    }

    pub fn notify(&self) {
        self.state.generation.set(self.state.generation.get() + 1);
        self.state.waiting.wake_all();
    }

    pub fn notify_result(&self, result: T) {
        *self.state.result.borrow_mut() = result;
        self.state.generation.set(self.state.generation.get() + 1);
        self.state.waiting.wake_all();
    }
}

impl<T> Repeated<T>
where
    T: Copy,
{
    pub fn new(value: T) -> Self {
        Self {
            state: Rc::new(RepeatedState::new(value)),
        }
    }
}

impl Default for Repeated<()> {
    fn default() -> Self {
        Self::new(())
    }
}

impl<T> Event<T> for Repeated<T>
where
    T: Copy + 'static,
{
    fn listen(&self) -> BoxFuture<'static, T> {
        Box::pin(RepeatedFuture {
            state: self.state.clone(),
            done: false,
            listener_id: None,
            observed_generation: self.state.generation.get(),
        })
    }

    /// Allow cloning of Boxed elements of vector
    fn clone_dyn(&self) -> Box<dyn Event<T>> {
        Box::new(self.clone())
    }
}

impl<T> Future for RepeatedFuture<T>
where
    T: Copy,
{
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.state.generation.get() > self.observed_generation {
            self.done = true;
            self.listener_id = None;
            Poll::Ready(*self.state.result.borrow())
        } else {
            if let Some(listener_id) = self.listener_id.take() {
                self.state.waiting.remove_listener(listener_id);
            }
            self.listener_id = Some(self.state.waiting.register_listener(cx.waker().clone()));
            Poll::Pending
        }
    }
}

impl<T> Drop for RepeatedFuture<T>
where
    T: Copy,
{
    fn drop(&mut self) {
        if !self.done
            && let Some(listener_id) = self.listener_id.take()
        {
            self.state.waiting.remove_listener(listener_id);
        }
    }
}
