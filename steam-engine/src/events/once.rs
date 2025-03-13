// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! An event that can only be triggered once

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use futures::Future;
use futures::future::FusedFuture;

use crate::sim_error;
use crate::traits::{BoxFuture, Event};
use crate::types::SimResult;

pub struct OnceState<T>
where
    T: Copy,
{
    listen_waiting: RefCell<Vec<Waker>>,
    triggered: RefCell<bool>,
    result: T,
}

impl<T> OnceState<T>
where
    T: Copy,
{
    pub fn new(value: T) -> Self {
        Self {
            listen_waiting: RefCell::new(Vec::new()),
            triggered: RefCell::new(false),
            result: value,
        }
    }
}

impl Default for OnceState<()> {
    fn default() -> Self {
        Self::new(())
    }
}

#[derive(Clone)]
pub struct Once<T>
where
    T: Copy,
{
    state: Rc<OnceState<T>>,
}

pub struct OnceFuture<T>
where
    T: Copy,
{
    state: Rc<OnceState<T>>,
    done: bool,
}

impl<T> FusedFuture for OnceFuture<T>
where
    T: Copy,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

impl<T> Once<T>
where
    T: Copy,
{
    pub fn with_value(value: T) -> Self {
        Self {
            state: Rc::new(OnceState::new(value)),
        }
    }

    pub fn notify(&self) -> SimResult {
        if *self.state.triggered.borrow() {
            sim_error!("once event already triggered")
        } else {
            *self.state.triggered.borrow_mut() = true;
            for waker in self.state.listen_waiting.borrow_mut().drain(..) {
                waker.wake();
            }
        }
        Ok(())
    }
}

impl<T> Once<T>
where
    T: Copy + 'static,
{
    pub fn new(value: T) -> Self {
        Self {
            state: Rc::new(OnceState::new(value)),
        }
    }
}

impl Default for Once<()> {
    fn default() -> Self {
        Self::new(())
    }
}

impl<T> Event<T> for Once<T>
where
    T: Copy + 'static,
{
    fn listen(&self) -> BoxFuture<'static, T> {
        Box::pin(OnceFuture {
            state: self.state.clone(),
            done: false,
        })
    }

    /// Allow cloning of Boxed elements of vector
    fn clone_dyn(&self) -> Box<dyn Event<T>> {
        Box::new(self.clone())
    }
}

impl<T> Future for OnceFuture<T>
where
    T: Copy,
{
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if *self.state.triggered.borrow() {
            self.done = true;
            Poll::Ready(self.state.result)
        } else {
            self.state
                .listen_waiting
                .borrow_mut()
                .push(cx.waker().clone());
            Poll::Pending
        }
    }
}
