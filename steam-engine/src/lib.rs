// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

// TODO: enable this warning to ensure all public interfaces are documented.
// Enable warnings for missing documentation
// #![warn(missing_docs)]

#![doc(test(attr(warn(unused))))]

//! `STEAM` - Simulation Technology for Evaluation and Architecture Modelling
//!
//! This library provides the core of the [STEAM Engine](crate::engine) which
//! executes event driven asynchronous simulation
//! [components](../steam_components/index.html).
//!
//! # Developer Guide
//!
//! The Developer Guide provides a document that goes through the STEAM engine
//! and related libraries in a more directed approach than the API guide can.
//! See the `steam-developer-guide/` folder.
//!
//! # Examples
//!
//! Make sure you look at the **examples/** folder which includes
//! worked/documented examples. The current examples are:
//!  - **examples/flaky-component**: a worked example of a simple two-port
//!    component.
//!  - **examples/flaky-with_delay**: a worked example of a simple two-port
//!    component that has some subcomponents.
//!  - **examples/scrambler**: a worked example of a component that registers a
//!    a vector of subcomponents.
//!
//! [components]: steam_components/index.html

//! # Simple Application
//!
//! A very simple application would look like:
//!
//! ```rust
//! use steam_components::sinks::Sink;
//! use steam_components::source::Source;
//! use steam_components::{connect_port, option_box_repeat};
//! use steam_engine::engine::Engine;
//! use steam_engine::run_simulation;
//!
//! let mut engine = Engine::default();
//! let mut source = Source::new(engine.top(), "source", option_box_repeat!(0x123 ; 10));
//! let sink = Sink::new(engine.top(), "sink");
//! connect_port!(source, tx => sink, rx);
//! run_simulation!(engine ; [source, sink]);
//! assert_eq!(sink.num_sunk(), 10);
//! ```

//! Simulations can be run as purely event driven (where one event triggers one
//! or more others) or the use of clocks can be introduced to model time. The
//! combination of both is the most common.
//!
//! The [engine](crate::engine::Engine) manages the
//! [clocks](crate::time::clock). A simple example of a component that uses the
//! clock is the
//! [rate limiter](../steam_components/flow_controls/rate_limiter/index.html)
//! which models the amount of time it takes for objects to pass through it.

pub mod engine;
pub mod events;
pub mod executor;
pub mod port;
pub mod test_helpers;
pub mod time;
pub mod traits;
pub mod types;

#[macro_export]
/// Spawn the run() for all components and provide read-only clones.
///
/// This allows the user to access the component through read-only functions
/// after the simulation has been run.
macro_rules! spawn_simulation {
    ($engine:ident ; $($iterable:ident),* $(,)?) => {
        $(
        let $iterable = $iterable.drain(..).map(|i| {
            let c = i.clone();
            $engine.spawn(async move { i.run().await });
            c
        }).collect::<Vec<_>>();

        // Get rid of any unused variable warnings
        let _ = $iterable;
        )*
    };
    ($engine:ident ; $($iterable:ident),* $(,)* [$($block:ident),* $(,)?]) => {
        $(
        let $iterable = $iterable.drain(..).map(|i| {
            let c = i.clone();
            $engine.spawn(async move { i.run().await });
            c
        }).collect::<Vec<_>>();

        // Get rid of any unused variable warnings
        let _ = $iterable;
        )*

        $(
        let clone = $block.clone();
        $engine.spawn(async move { $block.run().await } );
        let $block = clone;

        // Get rid of any unused variable warnings
        let _ = $block;
        )*
    };
}

#[macro_export]
/// Spawn all component run() functions and then run the simulation.
macro_rules! run_simulation {
    ($engine:ident) => {
        $engine.run().unwrap();
    };
    ($engine:ident ; $($iterable:ident),* $(,)?) => {
        $crate::spawn_simulation!($engine ; $($iterable,)*);
        $engine.run().unwrap();
    };
    ($engine:ident ; $($iterable:ident),* $(,)? [$($block:ident),* $(,)?]) => {
        $crate::spawn_simulation!($engine ; $($iterable,)* , [$($block,)*]);
        $engine.run().unwrap();
    };
    ($engine:ident, $expect:expr) => {
        match $engine.run() {
            Ok(()) => panic!("Expected an error!"),
            Err(e) => assert_eq!(format!("{e}").as_str(), $expect),
        }
    };
    ($engine:ident, $expect:expr ; $($iterable:ident),* $(,)?) => {
        $crate::spawn_simulation!($engine ; $($iterable,)*);
        match $engine.run() {
            Ok(()) => panic!("Expected an error!"),
            Err(e) => assert_eq!(format!("{e}").as_str(), $expect),
        }
    };
    ($engine:ident, $expect:expr ; $($iterable:ident),* $(,)? [$($block:ident),* $(,)?]) => {
        $crate::spawn_simulation!($engine ; $($iterable,)* , [$($block,)*]);
        match $engine.run() {
            Ok(()) => panic!("Expected an error!"),
            Err(e) => assert_eq!(format!("{e}").as_str(), $expect),
        }
    };
}

#[macro_export]
/// Spawn a sub-component that is stored in an `RefCell<Option<>>`
///
/// This removes the sub-component from the Option and then spawns the `run()`
/// function.
macro_rules! spawn_subcomponent {
    ($($spawner:ident).+ ; $($block:ident).+) => {
        let sub_block = $($block).+.borrow_mut().take().unwrap();
        $($spawner).+.spawn(async move { sub_block.run().await } );
    };
}
