// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! An event that can only be triggered once

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use futures::Future;
use futures::future::FusedFuture;

use super::waiting::Waiting;
use crate::sim_error;
use crate::traits::{BoxFuture, Event};
use crate::types::SimResult;

pub struct OnceState<T>
where
    T: Copy,
{
    waiting: Waiting,
    triggered: RefCell<bool>,
    result: T,
}

impl<T> OnceState<T>
where
    T: Copy,
{
    pub fn new(value: T) -> Self {
        Self {
            waiting: Waiting::new(),
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
    listener_id: Option<u64>,
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
            return sim_error!("once event already triggered");
        }
        *self.state.triggered.borrow_mut() = true;
        self.state.waiting.wake_all();
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
            listener_id: None,
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
            self.listener_id = None;
            Poll::Ready(self.state.result)
        } else {
            if let Some(listener_id) = self.listener_id.take() {
                self.state.waiting.remove_listener(listener_id);
            }
            self.listener_id = Some(self.state.waiting.register_listener(cx.waker().clone()));
            Poll::Pending
        }
    }
}

impl<T> Drop for OnceFuture<T>
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
