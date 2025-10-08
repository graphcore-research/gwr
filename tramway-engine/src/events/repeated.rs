// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! An event that can be triggered multiple times. The event allows
//! the notifier to pass a custom result to its listeners on each
//! notification, using the `notify_result()` method. Alternatively,
//! the last set result will be provided to the listeners. If no
//! result has been set, the default value for the result type will
//! be used.

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use futures::Future;
use futures::future::FusedFuture;

use crate::traits::{BoxFuture, Event};
use crate::types::SimResult;

pub struct RepeatedState<T>
where
    T: Copy,
{
    listen_waiting: RefCell<Vec<Waker>>,
    result: RefCell<T>,
}

impl<T> RepeatedState<T>
where
    T: Copy,
{
    pub fn new(value: T) -> Self {
        Self {
            listen_waiting: RefCell::new(Vec::new()),
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
    init: bool,
    done: bool,
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

    pub fn notify(&self) -> SimResult {
        for waker in self.state.listen_waiting.borrow_mut().drain(..) {
            waker.wake();
        }
        Ok(())
    }

    pub fn notify_result(&self, result: T) -> SimResult {
        *self.state.result.borrow_mut() = result;
        for waker in self.state.listen_waiting.borrow_mut().drain(..) {
            waker.wake();
        }
        Ok(())
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
            init: false,
            done: false,
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
        if self.init {
            self.done = true;
            Poll::Ready(*self.state.result.borrow())
        } else {
            self.init = true;
            self.state
                .listen_waiting
                .borrow_mut()
                .push(cx.waker().clone());
            Poll::Pending
        }
    }
}
