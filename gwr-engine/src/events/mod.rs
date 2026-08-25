// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Different types of events.
//!
//! Events should be used to coordinate between
//! [spawned](crate::executor::Spawner) tasks so that they can run in an
//! event-driven manner and yield until there is something ready to process.
//!
//! [Basic events](crate::events::once) are created to be triggered once using
//! `notify()` method. Any number of other tasks can be waiting for the event
//! to be triggered. The `listen()` method is used to wait for the event to be
//! triggered.
//!
//! The engine provides a small set of event types that cover the common
//! coordination patterns used by components and models:
//!
//! - [`Once`](crate::events::once::Once): fires exactly once and wakes every
//!   listener with a fixed result value. This is useful for completion,
//!   timeout, and one-off handshakes.
//! - [`Repeated`](crate::events::repeated::Repeated): can fire many times and
//!   wakes listeners waiting for the next generation. This is useful for state
//!   changes, monitor updates, and reusable notifications.
//! - [`AnyOf`](crate::events::any_of::AnyOf): combines several events and wakes
//!   when the first one fires, returning that event's result. This is useful
//!   for races such as response-or-timeout waits.
//! - [`AllOf`](crate::events::all_of::AllOf): combines several events and wakes
//!   once they have all fired. This is useful for joining setup, drain, or
//!   completion conditions.
//!
//! # Example:
//!
//! An event being created to co-ordinate between two tasks.
//!
//! ```rust
//! # use gwr_engine::engine::Engine;
//! # use gwr_engine::events::once::Once;
//! # use gwr_engine::run_simulation;
//! # use gwr_engine::traits::Event;
//! #
//! fn spawn_listen<T>(engine: &mut Engine, event: Once<T>)
//! where
//!     T: Copy + 'static,
//! {
//!     engine.spawn(async move {
//!         event.listen().await;
//!         println!("After event");
//!         Ok(())
//!     });
//! }
//!
//! fn spawn_notify<T>(engine: &mut Engine, event: Once<T>)
//! where
//!     T: Copy + 'static,
//! {
//!     let clock = engine.default_clock();
//!     engine.spawn(async move {
//!         clock.wait_ticks(10).await;
//!         println!("Trigger event");
//!         event.notify()?;
//!         Ok(())
//!     });
//! }
//!
//! fn main() {
//!     let mut engine = Engine::default();
//!     let event = Once::default();
//!     spawn_listen(&mut engine, event.clone());
//!     spawn_notify(&mut engine, event);
//!     run_simulation!(engine);
//!     # assert_eq!(engine.time_now_ns(), 10.0);
//! }
//! ```

pub mod all_of;
pub mod any_of;
pub mod once;
pub mod repeated;
mod waiting;
