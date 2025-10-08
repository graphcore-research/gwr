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
//! # Example:
//!
//! An event being created to co-ordinate between two tasks.
//!
//! ```rust
//! # use tramway_engine::engine::Engine;
//! # use tramway_engine::events::once::Once;
//! # use tramway_engine::run_simulation;
//! # use tramway_engine::traits::Event;
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
//!         event.notify();
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
