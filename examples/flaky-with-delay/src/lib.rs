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
//!  - One [input port](steam_engine::port::InPort): `rx`
//!  - One [output port](steam_engine::port::OutPort): `tx`

/// First need to `use` all types and traits that are used.
use std::cell::RefCell;
/// The `Rc` and `RefCell` libraries provide single-threaded sharing and
/// mutating of state.
use std::rc::Rc;
/// The `sync` part of the standard library brings in types used for thread
/// synchronisation.
use std::sync::Arc;

/// Random library is just used by this component to implement its drop
/// decisions.
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use steam_components::delay::Delay;
use steam_components::store::Store;
/// The steam components crate provides many connectable building blocks.
/// Component traits and types are provided along with the components
/// themselves.
use steam_components::{connect_port, connect_tx, port_rx, take_option};
/// The steam engine core provides the traits and types required to be
/// implemented by a component.
use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortState};
use steam_engine::spawn_subcomponent;
use steam_engine::time::clock::Clock;
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
/// The steam_track library provides tracing/logging features.
use steam_track::entity::Entity;
use steam_track::trace;

/// The overall structure for this compoment.
///
/// Note that in this example it is a *Generic* type in that it can be used in
/// a simulation of any type - as long as that type implements the `SimObject`
/// trait.
struct State<T>
where
    T: SimObject,
{
    /// This component has an `rx` port that it uses to handle incoming data.
    ///
    /// It is placed within an `Option` so that it can be removed later
    /// when the Engine is run.
    ///
    /// There is no need for an output port as the output is directly connected
    /// to the buffer's output.
    rx: RefCell<Option<InPort<T>>>,

    /// This flaky component uses a `Delay` component for any data that is
    /// not dropped. It is placed within an `Option` so that it can be removed
    /// later when the component is `run()`.
    delay: RefCell<Option<Delay<T>>>,

    /// After the `Delay` data will be placed into a `Store` from where it can
    /// be pulled and either passed on or dropped.
    /// It is again placed within an `Option` so that it can be removed later
    /// when the Engine is run.
    buffer: RefCell<Option<Store<T>>>,

    /// Store the ratio at which packets should be dropped.
    drop_ratio: f64,

    /// Random number generator used for deciding when to drop. Note that it is
    /// wrapped in a `Shared` which allows it to be used mutably in the `put()`
    /// function despite the fact that the Inner will be immutable (`&self`
    /// argument in the trait).
    rng: RefCell<StdRng>,
}

/// The next thing to do is define the generic functions for the new component.
impl<T> State<T>
where
    T: SimObject,
{
    /// In this case, the `new()` function creates the component from the
    /// parameters provided.
    pub fn new(
        entity: &Arc<Entity>,
        clock: Clock,
        spawner: Spawner,
        drop_ratio: f64,
        seed: u64,
        delay_ticks: usize,
    ) -> Self {
        let delay = Delay::new(entity, "delay", clock, spawner.clone(), delay_ticks);
        let buffer = Store::new(entity, "buffer", spawner, 1);

        connect_port!(delay, tx => buffer, rx);

        // Finally, create the component
        Self {
            drop_ratio,
            rx: RefCell::new(Some(InPort::new(entity, "rx"))),
            delay: RefCell::new(Some(delay)),
            buffer: RefCell::new(Some(buffer)),
            rng: RefCell::new(StdRng::seed_from_u64(seed)),
        }
    }

    /// Return the next random u32
    ///
    /// This is wrapped in a separate function to hide the interior mutation
    fn next_u32(&self) -> u32 {
        self.rng.borrow_mut().next_u32()
    }
}

/// A component needs to support being cloned and also being printed for debug
/// logging.
///
/// The `Clone` can be derived as long as all members support `Clone`. This is
/// why the state is wrapped in an `Rc`.
///
/// The `EntityDisply` automatically derives the `Display` trait as long as the
/// struct contains the `entity`.
#[derive(Clone, EntityDisplay)]
pub struct Flaky<T>
where
    T: SimObject,
{
    /// Every component should include an Entity that defines where in the
    /// overall simulation hierarchy it is. The Entity is also used to
    /// filter logging.
    pub entity: Arc<Entity>,

    /// Keep a reference to the `Spawner` in order to `run()` the subcomponents.
    spawner: Spawner,

    state: Rc<State<T>>,
}

/// The next thing to do is define the generic functions for the new component.
impl<T> Flaky<T>
where
    T: SimObject,
{
    /// In this case, the `new()` function creates the component from the
    /// parameters provided.
    pub fn new(
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        drop_ratio: f64,
        seed: u64,
        delay_ticks: usize,
    ) -> Self {
        // The entity needs to be created first because this component will be the
        // parent to the subcomponents.
        let entity = Entity::new(parent, name);

        // Because it is shared it needs to be wrapped in an Arc
        let entity = Arc::new(entity);

        let state = Rc::new(State::new(
            &entity,
            clock,
            spawner.clone(),
            drop_ratio,
            seed,
            delay_ticks,
        ));

        // Finally, create the component
        Self {
            entity,
            spawner,
            state,
        }
    }

    /// This provides the `InPort` to which you can connect
    pub fn port_rx(&self) -> Rc<PortState<T>> {
        // The `port_rx!` macro is the most consise way to access the rx port state.
        port_rx!(self.state.rx, state)
    }

    /// The ports of this component are effectively defined by the functions
    /// this component exposes. In this case, the `connect_port_tx` shows
    /// that this component has an `tx` port which should be connected to an
    /// `rx` port.
    ///
    /// In this case the `tx` port is connected directly to the buffer's `tx`
    /// port.
    pub fn connect_port_tx(&mut self, port_state: Rc<PortState<T>>) {
        // Because the State is immutable then we use the `connect_tx!` macro
        // in order to simplify the setup.
        connect_tx!(self.state.buffer, connect_port_tx ; port_state);
    }

    /// Implement the active aspect of thie component
    ///
    /// The `run()` function launches any sub-components and then performs the
    /// functionality of this component.
    pub async fn run(&self) -> SimResult {
        // Pull out the the `rx` port so that it is owned in this function.
        let rx = take_option!(self.state.rx);

        // Create a `tx` port and connect into the `delay` subcomponent
        let mut tx = OutPort::new(&self.entity, "delay_tx");
        tx.connect(port_rx!(self.state.delay, port_rx));

        // Spawn the subcomponents now they are connected
        spawn_subcomponent!(self.spawner ; self.state.delay);
        spawn_subcomponent!(self.spawner ; self.state.buffer);

        loop {
            // Receive a value from the input
            let value = rx.get().await;

            let next_u32 = self.state.next_u32();
            let ratio = next_u32 as f64 / u32::MAX as f64;
            if ratio > self.state.drop_ratio {
                // Only pass on a percentage of the data
                tx.put(value).await?;
            } else {
                // Let the user know this value has been dropped.
                trace!(self.entity ; "drop {}", value);
            }
        }
    }
}
