// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! Port

use std::cell::RefCell;
use std::fmt;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use futures::Future;
use futures::future::FusedFuture;
use steam_track::connect;
use steam_track::entity::Entity;

use crate::traits::SimObject;
use crate::types::SimResult;

pub struct PortState<T>
where
    T: SimObject,
{
    value: RefCell<Option<T>>,
    waiting_get: RefCell<Option<Waker>>,
    waiting_put: RefCell<Option<Waker>>,
    pub in_port_entity: Arc<Entity>,
}

impl<T> PortState<T>
where
    T: SimObject,
{
    fn new(in_port_entity: Arc<Entity>) -> Self {
        Self {
            value: RefCell::new(None),
            waiting_get: RefCell::new(None),
            waiting_put: RefCell::new(None),
            in_port_entity,
        }
    }
}

pub struct InPort<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    state: Rc<PortState<T>>,
    connected: RefCell<bool>,
}

impl<T> fmt::Display for InPort<T>
where
    T: SimObject,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.entity.fmt(f)
    }
}

impl<T> InPort<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new(parent: &Arc<Entity>, name: &str) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        Self {
            entity: entity.clone(),
            state: Rc::new(PortState::new(entity)),
            connected: RefCell::new(false),
        }
    }

    pub fn state(&self) -> Rc<PortState<T>> {
        if *self.connected.borrow() {
            panic!("{self} already connected");
        }

        *self.connected.borrow_mut() = true;
        self.state.clone()
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn get(&self) -> PortGet<T> {
        if !*self.connected.borrow() {
            panic!("{self} not connected");
        }

        PortGet {
            state: self.state.clone(),
            done: false,
        }
    }

    /// Must be matched with a `finish_get` to allow the OutPort to continue.
    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn start_get(&self) -> PortStartGet<T> {
        if !*self.connected.borrow() {
            panic!("{self} not connected");
        }

        PortStartGet {
            state: self.state.clone(),
            done: false,
        }
    }

    /// Must be matched with a `start_get ` to consume the value.
    pub fn finish_get(&self) {
        if let Some(waker) = self.state.waiting_put.borrow_mut().take() {
            waker.wake();
        }
    }
}

pub struct OutPort<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    state: Option<Rc<PortState<T>>>,
}

impl<T> fmt::Display for OutPort<T>
where
    T: SimObject,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.entity.fmt(f)
    }
}

impl<T> OutPort<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new(parent: &Arc<Entity>, name: &str) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        Self {
            entity,
            state: None,
        }
    }

    pub fn connect(&mut self, port_state: Rc<PortState<T>>) {
        connect!(self.entity ; port_state.in_port_entity);
        match self.state {
            Some(_) => panic!("{} already connected", self.entity),
            None => {
                self.state = Some(port_state);
            }
        }
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn put(&self, value: T) -> PortPut<T> {
        let state = self
            .state
            .as_ref()
            .unwrap_or_else(|| panic!("{self} not connected"))
            .clone();
        PortPut {
            state,
            value: RefCell::new(Some(value)),
            done: RefCell::new(false),
        }
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn try_put(&self) -> PortTryPut<T> {
        let state = self
            .state
            .as_ref()
            .unwrap_or_else(|| panic!("{self} not connected"))
            .clone();
        PortTryPut { state, done: false }
    }
}

pub struct PortPut<T>
where
    T: SimObject,
{
    state: Rc<PortState<T>>,
    value: RefCell<Option<T>>,
    done: RefCell<bool>,
}

impl<T> Future for PortPut<T>
where
    T: SimObject,
{
    type Output = SimResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.value.take() {
            Some(value) => {
                // The state is designed to be shared between one put/get pair so it should
                // not be possible for the value in the state to be set at this point.
                assert!(self.state.value.borrow().is_none());

                *self.state.value.borrow_mut() = Some(value);
                if let Some(waker) = self.state.waiting_get.borrow_mut().take() {
                    waker.wake();
                }
                *self.state.waiting_put.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
            None => {
                // Value already sent, woken because it has been consumed
                *self.done.borrow_mut() = true;
                Poll::Ready(Ok(()))
            }
        }
    }
}

impl<T> FusedFuture for PortPut<T>
where
    T: SimObject,
{
    fn is_terminated(&self) -> bool {
        *self.done.borrow()
    }
}

pub struct PortTryPut<T>
where
    T: SimObject,
{
    state: Rc<PortState<T>>,
    done: bool,
}

impl<T> Future for PortTryPut<T>
where
    T: SimObject,
{
    type Output = SimResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.state.waiting_get.borrow().is_some() {
            self.done = true;
            Poll::Ready(Ok(()))
        } else {
            *self.state.waiting_put.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl<T> FusedFuture for PortTryPut<T>
where
    T: SimObject,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

pub struct PortGet<T>
where
    T: SimObject,
{
    state: Rc<PortState<T>>,
    done: bool,
}

impl<T> Future for PortGet<T>
where
    T: SimObject,
{
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let value = self.state.value.borrow_mut().take();
        if let Some(value) = value {
            self.done = true;

            if let Some(waker) = self.state.waiting_put.borrow_mut().take() {
                waker.wake();
            }
            Poll::Ready(value)
        } else {
            if let Some(waker) = self.state.waiting_put.borrow_mut().take() {
                waker.wake();
            }

            *self.state.waiting_get.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl<T> FusedFuture for PortGet<T>
where
    T: SimObject,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

pub struct PortStartGet<T>
where
    T: SimObject,
{
    state: Rc<PortState<T>>,
    done: bool,
}

impl<T> Future for PortStartGet<T>
where
    T: SimObject,
{
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let value = self.state.value.borrow_mut().take();
        if let Some(value) = value {
            self.done = true;
            Poll::Ready(value)
        } else {
            *self.state.waiting_get.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl<T> FusedFuture for PortStartGet<T>
where
    T: SimObject,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}
