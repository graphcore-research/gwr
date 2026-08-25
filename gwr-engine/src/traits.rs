// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A set of common traits used across GWR Engine.

use core::mem::size_of;
use std::fmt::{Debug, Display};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_track::id::Unique;

use crate::types::{AccessType, DeviceId, SimResult};

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
    fn dst_addr(&self) -> u64;
    fn src_addr(&self) -> u64;
    fn dst_device(&self) -> DeviceId {
        DeviceId(self.dst_addr())
    }
    fn src_device(&self) -> DeviceId {
        DeviceId(self.src_addr())
    }
    fn access_type(&self) -> AccessType;
}

/// A super-trait that objects that are passed around the simulation have to
/// implement
///
/// This is the minimum object contract required by ports, flow controls,
/// tracking, and most test harnesses. The trait intentionally combines data
/// movement concerns (`Clone`, `Unpin`, `'static`), diagnostics (`Debug`,
/// `Display`), observability (`Unique`), and bandwidth modelling
/// (`TotalBytes`). Components that route objects require [`Routable`]
/// separately.
///
///  - `Clone`: Allows applications and components to retain copies of values
///    sent through the simulation, including values such as `Vec` that are not
///    `Copy`.
///  - `Debug` and `Display`: Support diagnostic and trace output.
///  - `Unique`: Supplies the value's tracking ID.
///  - `TotalBytes`: Supplies the size used by bandwidth and rate models.
///  - `Unpin`: Allows values to be moved out of port futures safely.
///  - `'static`: Allows port futures containing values to be owned by spawned
///    simulation tasks.
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
    fn dst_addr(&self) -> u64 {
        *self as u64
    }
    fn src_addr(&self) -> u64 {
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
    fn dst_addr(&self) -> u64 {
        *self as u64
    }
    fn src_addr(&self) -> u64 {
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
/// Components with no independent async behavior can use the default
/// implementation. Active components need to override [`run`](Runnable::run).
/// Because the executor is single-threaded, active components normally share
/// state through `Rc`, `RefCell`, and `Cell` instead of `Arc` or locks. Ports
/// are often stored as `RefCell<Option<...>>`: setup code connects them through
/// `&self`, and `run` takes ownership of them for the lifetime of the spawned
/// task.
///
/// The `#[async_trait(?Send)]` decorator keeps the trait usable through
/// `dyn Runnable` and produces futures suitable for the single-threaded
/// executor. A basic implementation of the trait looks like:
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
/// A default implementation is provided for any component that doesn't have any
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
        assert_eq!(7_i32.dst_addr(), 7);
        assert_eq!(7_i32.src_addr(), 7);
        assert_eq!(7_i32.dst_device(), DeviceId(7));
        assert_eq!(7_i32.src_device(), DeviceId(7));
        assert_eq!(0_i32.access_type(), AccessType::ReadRequest);
        assert_eq!(1_i32.access_type(), AccessType::WriteRequest);
        assert_eq!(2_i32.access_type(), AccessType::WriteNonPostedRequest);
        assert_eq!(3_i32.access_type(), AccessType::ReadResponse);
        assert_eq!(4_i32.access_type(), AccessType::WriteNonPostedResponse);
        assert_eq!(5_i32.access_type(), AccessType::Control);

        assert_eq!(0_usize.total_bytes(), size_of::<usize>());
        assert_eq!(7_usize.dst_addr(), 7);
        assert_eq!(7_usize.src_addr(), 7);
        assert_eq!(7_usize.dst_device(), DeviceId(7));
        assert_eq!(7_usize.src_device(), DeviceId(7));
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
