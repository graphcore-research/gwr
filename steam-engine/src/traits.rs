// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A set of common traits used across STEAM Engine.

use core::mem::size_of;
use std::fmt::{Debug, Display};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use steam_track::tag::Tagged;

use crate::types::ReqType;

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
    fn dest(&self) -> u64;
    fn req_type(&self) -> ReqType;
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
///  - TotalBytes:  Allows rate limiting.
///  - Tagged:      Allows for simple logging
///  - 'static:     Due to the way that futures are implemented, the lifetimes
///    need to be `static. This means that objects may have to be placed in
///    `Box` to make the static.
pub trait SimObject: Clone + Debug + Display + Routable + Tagged + TotalBytes + 'static {}

// Implementations for basic types that can be sent around the simulation for
// testing

// i32
impl TotalBytes for i32 {
    fn total_bytes(&self) -> usize {
        size_of::<i32>()
    }
}

impl Routable for i32 {
    fn dest(&self) -> u64 {
        *self as u64
    }
    fn req_type(&self) -> ReqType {
        match self {
            0 => ReqType::Read,
            1 => ReqType::Write,
            2 => ReqType::WriteNonPosted,
            3 => ReqType::Control,
            _ => {
                panic!()
            }
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
    fn dest(&self) -> u64 {
        *self as u64
    }
    fn req_type(&self) -> ReqType {
        match self {
            0 => ReqType::Read,
            1 => ReqType::Write,
            2 => ReqType::WriteNonPosted,
            3 => ReqType::Control,
            _ => {
                panic!()
            }
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
