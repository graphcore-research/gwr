// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This is an example component that delays data and also randomly drop data.
//!
//! This component uses the existing `Delay` and `Store` components.
//!
//! The `main.rs` in this folder shows how it can be used.
//!
//! # Ports
//!
//! This component has two ports
//!  - One [input port](gwr_engine::port::InPort): `rx`
//!  - One [output port](gwr_engine::port::OutPort): `tx`

/// First need to `use` all types and traits that are used.
use std::cell::RefCell;
/// The `Rc` and `RefCell` libraries provide single-threaded sharing and
/// mutating of state.
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::delay::Delay;
use gwr_components::store::Store;
/// The gwr components crate provides many connectable building blocks.
/// Component traits and types are provided along with the components
/// themselves.
use gwr_components::{connect_tx, port_rx, take_option};
use gwr_engine::engine::Engine;
/// The gwr engine core provides the traits and types required to be
/// implemented by a component.
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Runnable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::EntityDisplay;
/// The gwr_track library provides tracing/logging features.
use gwr_track::entity::Entity;
use gwr_track::trace;
/// Random library is just used by this component to implement its drop
/// decisions.
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

/// A struct containing configuration options for the component
pub struct Config {
    /// Ratio for how many packets are dropped (in the range [0, 1])
    drop_ratio: f64,

    /// Seed for random number generator
    seed: u64,

    /// Delay in clock ticks
    delay_ticks: usize,
}

impl Config {
    #[must_use]
    pub fn new(drop_ratio: f64, seed: u64, delay_ticks: usize) -> Self {
        assert!((0.0..=1.0).contains(&drop_ratio));
        Self {
            drop_ratio,
            seed,
            delay_ticks,
        }
    }
}

/// A component needs to support being cloned and also being printed for debug
/// logging.
///
/// The `EntityDisply` automatically derives the `Display` trait as long as the
/// struct contains the `entity`.
#[derive(EntityDisplay)]
pub struct Flaky<T>
where
    T: SimObject,
{
    /// Every component should include an Entity that defines where in the
    /// overall simulation hierarchy it is. The Entity is also used to
    /// filter logging.
    pub entity: Rc<Entity>,

    /// This component has an `rx` port that it uses to handle incoming data.
    ///
    /// It is placed within an `Option` so that it can be removed later
    /// when the Engine is run.
    rx: RefCell<Option<InPort<T>>>,

    /// This component has an internal `tx` port is connected to the delay.
    /// Any data that is not dropped is sent through this port.
    ///
    /// It is placed within an `Option` so that it can be removed later
    /// when the Engine is run.
    tx: RefCell<Option<OutPort<T>>>,

    /// After the `Delay` data will be placed into a `Store` from where it can
    /// be pulled and either passed on or dropped.
    ///
    /// It is again placed within an `Option` so that it can be removed later
    /// when the Engine is run.
    buffer: RefCell<Option<Rc<Store<T>>>>,

    /// Store the ratio at which packets should be dropped.
    drop_ratio: f64,

    /// Random number generator used for deciding when to drop. Note that it is
    /// wrapped in a `Shared` which allows it to be used mutably in the `put()`
    /// function despite the fact that the Inner will be immutable (`&self`
    /// argument in the trait).
    rng: RefCell<StdRng>,
}

/// The next thing to do is define the generic functions for the new component.
impl<T> Flaky<T>
where
    T: SimObject,
{
    /// In this case, the `new_and_register()` function creates the component
    /// from the parameters provided as well as registering the component
    /// with the `Engine`.
    pub fn new_and_register(
        engine: &Engine,
        parent: &Rc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        config: &Config,
    ) -> Result<Rc<Self>, SimError> {
        // The entity needs to be created first because this component will be the
        // parent to the subcomponents.
        let entity = Entity::new(parent, name);

        // Because it is shared it needs to be wrapped in an Arc
        let entity = Rc::new(entity);

        let delay = Delay::new_and_register(
            engine,
            &entity,
            "delay",
            clock,
            spawner.clone(),
            config.delay_ticks,
        )?;
        let buffer = Store::new_and_register(engine, &entity, "buffer", spawner, 1)?;

        delay.connect_port_tx(buffer.port_rx())?;

        // Create an internal `tx` port and connect into the `delay` subcomponent
        let mut tx = OutPort::new(&entity, "delay_tx");
        tx.connect(delay.port_rx())?;

        let rx = InPort::new(&entity, "rx");

        // Finally, create the component
        let rc_self = Rc::new(Self {
            entity,
            drop_ratio: config.drop_ratio,
            rx: RefCell::new(Some(rx)),
            tx: RefCell::new(Some(tx)),
            buffer: RefCell::new(Some(buffer)),
            rng: RefCell::new(StdRng::seed_from_u64(config.seed)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    /// This provides the `InPort` to which you can connect
    pub fn port_rx(&self) -> PortStateResult<T> {
        // The `port_rx!` macro is the most consise way to access the rx port state.
        port_rx!(self.rx, state)
    }

    /// The ports of this component are effectively defined by the functions
    /// this component exposes. In this case, the `connect_port_tx` shows
    /// that this component has an `tx` port which should be connected to an
    /// `rx` port.
    ///
    /// In this case the `tx` port is connected directly to the buffer's `tx`
    /// port.
    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        // Because the State is immutable then we use the `connect_tx!` macro
        // in order to simplify the setup.
        connect_tx!(self.buffer, connect_port_tx ; port_state)
    }

    /// Return the next random u32
    ///
    /// This is wrapped in a separate function to hide the interior mutation
    fn next_u32(&self) -> u32 {
        self.rng.borrow_mut().next_u32()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Flaky<T>
where
    T: SimObject,
{
    /// Implement the active aspect of thie component
    ///
    /// The `run()` function launches any sub-components and then performs the
    /// functionality of this component.
    async fn run(&self) -> SimResult {
        // Pull out the `rx` port so that it is owned in this function.
        let rx = take_option!(self.rx);

        // Pull out the internal `tx` port so that it is owned in this function.
        let tx = take_option!(self.tx);

        loop {
            // Receive a value from the input
            let value = rx.get()?.await;

            let next_u32 = self.next_u32();
            let ratio = next_u32 as f64 / u32::MAX as f64;
            if ratio > self.drop_ratio {
                // Only pass on a percentage of the data
                tx.put(value)?.await;
            } else {
                // Let the user know this value has been dropped.
                trace!(self.entity ; "drop {}", value);
            }
        }
    }
}
