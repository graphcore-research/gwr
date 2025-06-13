// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! An event that is triggered when all events in the provided set are
//! triggered.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use futures::StreamExt;
use futures::future::{FusedFuture, LocalBoxFuture};
use futures::stream::FuturesUnordered;

use crate::traits::{BoxFuture, Event};
use crate::types::Eventable;

pub struct AllOfState<T> {
    all_of: RefCell<Vec<Eventable<T>>>,
}

impl<T> AllOfState<T> {
    #[must_use]
    pub fn new(all_of: Vec<Eventable<T>>) -> Self {
        Self {
            all_of: RefCell::new(all_of),
        }
    }
}

pub struct AllOf<T> {
    state: Rc<AllOfState<T>>,
}

impl<T> AllOf<T> {
    #[must_use]
    pub fn new(all_of: Vec<Eventable<T>>) -> Self {
        Self {
            state: Rc::new(AllOfState::new(all_of)),
        }
    }
}

impl<T> Clone for AllOf<T> {
    fn clone(&self) -> Self {
        let all_of = self.state.all_of.borrow();
        let cloned = all_of.to_vec();
        Self {
            state: Rc::new(AllOfState::new(cloned)),
        }
    }
}

impl<T> Event<T> for AllOf<T>
where
    T: Default + 'static,
{
    fn listen(&self) -> BoxFuture<'static, T> {
        Box::pin(AllOfFuture {
            state: self.state.clone(),
            future: None,
            done: false,
        })
    }

    /// Allow cloning of Boxed elements of vector
    fn clone_dyn(&self) -> Box<dyn Event<T>> {
        Box::new(self.clone())
    }
}

pub struct AllOfFuture<T>
where
    T: Default,
{
    state: Rc<AllOfState<T>>,
    future: Option<LocalBoxFuture<'static, ()>>,
    done: bool,
}

impl<T> FusedFuture for AllOfFuture<T>
where
    T: Default + 'static,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

impl<T> Future for AllOfFuture<T>
where
    T: Default + 'static,
{
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.future.is_none() {
            let events = self.state.all_of.borrow_mut().drain(..).collect();
            let future = all_of(events);
            let pinned_future = Box::pin(future);
            self.future = Some(pinned_future);
        }

        let future = self.future.as_mut().unwrap();
        match future.as_mut().poll(cx) {
            Poll::Ready(_) => {
                self.done = true;
                Poll::Ready(T::default())
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

async fn all_of<T: Default>(events: Vec<Box<dyn Event<T>>>) {
    let mut futures = FuturesUnordered::new();
    for e in &events {
        futures.push(e.listen());
    }

    while (futures.next().await).is_some() {
        // Keep waiting till all are done
    }
}
