// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A set of common traits used across GWR Engine.

use core::mem::size_of;
use std::fmt::{Debug, Display};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_track::id::Unique;

use crate::types::{AccessType, SimResult};

/// The `TotalBytes` trait is used to determine how many bytes an object
/// represents
///
/// This trait is used to determine how much time an object will take to be
/// sent.
pub trait TotalBytes {
    fn total_bytes(&self) -> usize;
}

/// The `Routable` trait provides an interface to an object to enable it to be
/// routed
pub trait Routable {
    fn destination(&self) -> u64;
    fn access_type(&self) -> AccessType;
}

/// A super-trait that objects that are passed around the simulation have to
/// implement
///
///  - Clone:       It would be nice to use `Copy` instead, but given that
///    things like `Vec` are not `Copy` we have to use `Clone` instead to allow
///    the application to keep copies of objects sent around.
///  - Debug:       In order to print "{:?}" objects have to at least implement
///    Debug. We could require Display, but that requires explicit
///    implementation.
///  - Routable:    Allows routing.
///  - Unique:      Allows for unique identification of `Entities`.
///  - TotalBytes:  Allows rate limiting.
///  - Unpin:       Required in order to be able to Unpin in port futures.
///  - 'static:     Due to the way that futures are implemented, the lifetimes
///    need to be `static. This means that objects may have to be placed in
///    `Box` to make the static.
pub trait SimObject: Clone + Debug + Display + Unique + TotalBytes + Unpin + 'static {}

// Implementations for basic types that can be sent around the simulation for
// testing

// i32
impl TotalBytes for i32 {
    fn total_bytes(&self) -> usize {
        size_of::<i32>()
    }
}

impl Routable for i32 {
    fn destination(&self) -> u64 {
        *self as u64
    }
    fn access_type(&self) -> AccessType {
        match self {
            0 => AccessType::ReadRequest,
            1 => AccessType::WriteRequest,
            2 => AccessType::WriteNonPostedRequest,
            3 => AccessType::ReadResponse,
            4 => AccessType::WriteNonPostedResponse,
            _ => AccessType::Control,
        }
    }
}

impl SimObject for i32 {}

// usize
impl TotalBytes for usize {
    fn total_bytes(&self) -> usize {
        size_of::<usize>()
    }
}

impl Routable for usize {
    fn destination(&self) -> u64 {
        *self as u64
    }
    fn access_type(&self) -> AccessType {
        match self {
            0 => AccessType::ReadRequest,
            1 => AccessType::WriteRequest,
            2 => AccessType::WriteNonPostedRequest,
            3 => AccessType::ReadResponse,
            4 => AccessType::WriteNonPostedResponse,
            _ => AccessType::Control,
        }
    }
}

impl SimObject for usize {}

/// The `Event` trait defines an object that can be used as an Event
///
/// This is a trait that defines the `listen` function that returns a future
/// so that it can be used in `async` code.
///
/// ```rust
/// use futures::future::BoxFuture;
/// pub trait Event<T> {
///     fn listen(&self) -> BoxFuture<'static, T>;
/// }
/// ```
pub trait Event<T> {
    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    fn listen(&self) -> BoxFuture<'static, T>;

    /// Allow cloning of Boxed elements of vector for AllOf/AnyOf
    ///
    /// See [stack overflow post](https://stackoverflow.com/questions/69890183/how-can-i-clone-a-vecboxdyn-trait)
    fn clone_dyn(&self) -> Box<dyn Event<T>>;
}

/// Provide Clone implementation for boxed Event
impl<T> Clone for Box<dyn Event<T>> {
    fn clone(self: &Box<dyn Event<T>>) -> Box<dyn Event<T>> {
        self.clone_dyn()
    }
}

/// Complete any pending transactions.
pub trait Resolve {
    /// Complete any pending update.
    fn resolve(&self);
}

/// A [`Resolver`] is used to register any [`Resolve`] functions that need to be
/// called.
pub trait Resolver {
    fn add_resolve(&self, resolve: Rc<dyn Resolve + 'static>);
}

pub type BoxFuture<'a, T> = Pin<std::boxed::Box<dyn Future<Output = T> + 'a>>;

/// The `Runnable` trait defines any active functionality that is spawned by a
/// component.
///
/// This is a trait that defines an `async` function and therefore currently
/// needs to use the `#[async_trait(?Send)]` decorator that converts it to a
/// pinned boxed result. A basic implementation of the trait looks like:
///
/// ```rust
/// # use gwr_engine::types::SimResult;
/// # use async_trait::async_trait;
/// #[async_trait(?Send)]
/// pub trait Runnable {
///     async fn run(&self) -> SimResult {
///         Ok(())
///     }
/// }
/// ```
///
/// A default implementation is provided for any compoment that doesn't have any
/// active behaviour.
#[async_trait(?Send)]
pub trait Runnable {
    /// Provides the method that defines the active element of this component.
    ///
    /// Default implementation is to do nothing.
    async fn run(&self) -> SimResult {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests added simply for code coverage
    #[test]
    fn integer_sim_object_defaults_are_available() {
        assert_eq!(0_i32.total_bytes(), size_of::<i32>());
        assert_eq!(7_i32.destination(), 7);
        assert_eq!(0_i32.access_type(), AccessType::ReadRequest);
        assert_eq!(1_i32.access_type(), AccessType::WriteRequest);
        assert_eq!(2_i32.access_type(), AccessType::WriteNonPostedRequest);
        assert_eq!(3_i32.access_type(), AccessType::ReadResponse);
        assert_eq!(4_i32.access_type(), AccessType::WriteNonPostedResponse);
        assert_eq!(5_i32.access_type(), AccessType::Control);

        assert_eq!(0_usize.total_bytes(), size_of::<usize>());
        assert_eq!(7_usize.destination(), 7);
        assert_eq!(0_usize.access_type(), AccessType::ReadRequest);
        assert_eq!(1_usize.access_type(), AccessType::WriteRequest);
        assert_eq!(2_usize.access_type(), AccessType::WriteNonPostedRequest);
        assert_eq!(3_usize.access_type(), AccessType::ReadResponse);
        assert_eq!(4_usize.access_type(), AccessType::WriteNonPostedResponse);
        assert_eq!(5_usize.access_type(), AccessType::Control);
    }

    struct PassiveRunnable;

    #[test]
    fn runnable_default_run_completes_successfully() {
        let runnable = PassiveRunnable;

        futures::executor::LocalPool::new()
            .run_until(runnable.run())
            .unwrap();
    }

    #[async_trait(?Send)]
    impl Runnable for PassiveRunnable {}
}
