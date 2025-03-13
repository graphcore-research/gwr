// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! An event that is triggered when one event in the provided set is triggered.
//!
//! The [AnyOf] will also return the data associated with the event that fired.
//!
//! # Example
//!
//! Here is a basic example of creating a custom enum and using it to handle
//! different events in different ways.
//!
//! ```rust
//! # use steam_engine::engine::Engine;
//! # use steam_engine::events::once::Once;
//! # use steam_engine::events::any_of::AnyOf;
//! # use steam_engine::traits::Event;
//! #
//! // Create the `enum` that defines all values returned by the event. It must
//! // implement `Clone` and `Copy`
//! #[derive(Clone, Copy)]
//! enum EventResult {
//!     TimedOut,
//!     AllOk,
//!     // ... other results
//! }
//! #
//! # let mut engine = Engine::default();
//! #
//! // Create the events
//! let timeout = Once::new(EventResult::TimedOut);
//! let ok = Once::new(EventResult::AllOk);
//! let anyof = AnyOf::new(vec![Box::new(timeout.clone()), Box::new(ok.clone())]);
//!
//! // Spawn a task that will trigger the timeout
//! # let clock = engine.default_clock();
//! engine.spawn(async move {
//!     clock.wait_ticks(10000).await;
//!     timeout.notify();
//!     Ok(())
//! });
//!
//! // Spawn a task that will say all is ok
//! # let clock = engine.default_clock();
//! engine.spawn(async move {
//!     clock.wait_ticks(1000).await;
//!     ok.notify();
//!     Ok(())
//! });
//!
//! // Handle the events
//! engine.spawn(async move {
//!     match anyof.listen().await {
//!         EventResult::TimedOut => panic!("Timed out"),
//!         EventResult::AllOk => println!("All Ok"),
//!         // ...
//!     }
//!     Ok(())
//! });
//!
//! # engine.run().unwrap();
//! ```

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

pub struct AnyOfState<T> {
    any_of: RefCell<Vec<Eventable<T>>>,
}

impl<T> AnyOfState<T> {
    pub fn new(any_of: Vec<Eventable<T>>) -> Self {
        Self {
            any_of: RefCell::new(any_of),
        }
    }
}

pub struct AnyOf<T> {
    state: Rc<AnyOfState<T>>,
}

impl<T> AnyOf<T> {
    pub fn new(any_of: Vec<Eventable<T>>) -> Self {
        Self {
            state: Rc::new(AnyOfState::new(any_of)),
        }
    }
}

impl<T> Clone for AnyOf<T> {
    fn clone(&self) -> Self {
        let any_of = self.state.any_of.borrow();
        let cloned = any_of.to_vec();
        Self {
            state: Rc::new(AnyOfState::new(cloned)),
        }
    }
}

impl<T> Event<T> for AnyOf<T>
where
    T: 'static,
{
    fn listen(&self) -> BoxFuture<'static, T> {
        Box::pin(AnyOfFuture {
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

pub struct AnyOfFuture<T> {
    state: Rc<AnyOfState<T>>,
    future: Option<LocalBoxFuture<'static, T>>,
    done: bool,
}

impl<T> FusedFuture for AnyOfFuture<T>
where
    T: 'static,
{
    fn is_terminated(&self) -> bool {
        self.done
    }
}

impl<T> Future for AnyOfFuture<T>
where
    T: 'static,
{
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.future.is_none() {
            let events = self.state.any_of.borrow_mut().drain(..).collect();
            let future = any_of(events);
            let pinned_future = Box::pin(future);
            self.future = Some(pinned_future);
        }

        let future = self.future.as_mut().unwrap();
        match future.as_mut().poll(cx) {
            Poll::Ready(value) => {
                self.done = true;
                Poll::Ready(value)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

async fn any_of<T>(events: Vec<Box<dyn Event<T>>>) -> T {
    let mut futures = FuturesUnordered::new();
    for e in events.iter() {
        futures.push(e.listen());
    }

    futures.next().await.unwrap()
}
