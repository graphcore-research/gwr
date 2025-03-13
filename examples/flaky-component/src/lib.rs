// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This is an example component that will randomly drop data being passed
//! through it.
//!
//! The `main.rs` in this folder shows how it can be used.
//!
//! # Ports
//!
//! This component has two ports
//!  - One [input port](steam_engine::port::InPort): `rx`
//!  - One [output put port](steam_engine::port::OutPort): `tx`

// ANCHOR: use

/// The `RefCell` allows the engine to be able to access state immutably as
/// well as mutably.
use std::cell::RefCell;
/// The `Rc` part of the standard library brings in types used for thread
/// synchronisation.
use std::rc::Rc;
/// Required by the shared Entities
use std::sync::Arc;

/// Random library is just used by this component to implement its drop
/// decisions.
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use steam_components::{connect_tx, port_rx, take_option};
use steam_engine::port::{InPort, OutPort, PortState};
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
/// The steam_track library provides tracing/logging features.
use steam_track::entity::Entity;
use steam_track::trace;

// ANCHOR_END: use

// ANCHOR: struct

/// The overall structure for this compoment.
///
/// Note that in this example it is a *Generic* type in that it can be used in
/// a simulation of any type - as long as that type implements the `SimObject`
/// trait.
///
/// The `fmt::Display` trait is used when converting a component to a string
/// for logging/printing using "{}". Simply pass through to the entity. This can
/// be hand-written, but the `EntityDisplay` derive writes this automatically.
#[derive(Clone, EntityDisplay)]
pub struct Flaky<T>
where
    T: SimObject,
{
    /// Every component should include an Entity that defines where in the
    /// overall simulation hierarchy it is. The Entity is also used to
    /// filter logging.
    pub entity: Arc<Entity>,

    /// In order to be able to share references to this component all members
    /// have to support `Clone` and hence the state is wrapped in an `Rc`. This
    /// makes is immutable (read-only).
    state: Rc<State<T>>,
}
// ANCHOR_END: struct

// ANCHOR: state

/// The State is the part of the component that wraps up all ports and mutable
/// state.
struct State<T>
where
    T: SimObject,
{
    /// Store the ratio at which packets should be dropped.
    drop_ratio: f64,

    /// Random number generator used for deciding when to drop. Note that it is
    /// wrapped in a `RefCell` which allows it to be used mutably in the `put()`
    /// function despite the fact that the State will be immutable (`&self`
    /// argument in the trait.
    rng: RefCell<StdRng>,

    /// Rx port to which to send any data that hasn't been dropped.
    /// Again, needs to be wrapped in the `Shared` to allow it to be changed
    /// when components are actually connected.
    ///
    /// Note: It is also wrapped in an Option so that it can be take out in the
    /// run() function.
    rx: RefCell<Option<InPort<T>>>,

    /// Tx port to which to send any data that hasn't been dropped.
    /// Again, needs to be wrapped in the `RefCell` to allow it to be changed
    /// when components are actually connected.
    ///
    /// Note: It is also wrapped in an Option so that it can be take out in the
    /// run() function.
    tx: RefCell<Option<OutPort<T>>>,
}
// ANCHOR_END: state

// ANCHOR: implFlaky

/// The next thing to do is define the generic functions for the new component.
impl<T> Flaky<T>
where
    T: SimObject,
{
    /// In this case, the `new()` function creates the component from the
    /// parameters provided.
    pub fn new(parent: &Arc<Entity>, name: &str, drop_ratio: f64, seed: u64) -> Self {
        // The entity needs to be created first because it is shared between the state
        // and the component itself.
        let entity = Entity::new(parent, name);

        // Because it is shared it needs to be wrapped in an Arc
        let entity = Arc::new(entity);

        // The State is then created and passed the paramaters defining is
        // functionality.
        let state = State {
            // Note that the entity needs to be `clone`d so that it can be passed
            // to multiple places.
            drop_ratio,
            rng: RefCell::new(StdRng::seed_from_u64(seed)),
            rx: RefCell::new(Some(InPort::new(&entity, "rx"))),
            tx: RefCell::new(Some(OutPort::new(&entity, "tx"))),
        };

        // Finally, the top-level struct is created with the `State` wrapped in an Rc.
        Self {
            entity,
            state: Rc::new(state),
        }
    }

    /// This provides the `InPort` to which you can connect
    pub fn port_rx(&self) -> Rc<PortState<T>> {
        // The `port_rx!` macro is the most consise way to access the rx port state.
        port_rx!(self.state.rx, state)
    }

    /// The ports of this component are effectively defined by the functions
    /// this component exposes. In this case, the `connect_port_tx` shows
    /// that this component has an TX port which should be connected to an RX
    /// port.
    pub fn connect_port_tx(&mut self, port_state: Rc<PortState<T>>) {
        // Because the State is immutable then we use the `connect_tx!` macro
        // in order to simplify the setup.
        connect_tx!(self.state.tx, connect ; port_state);
    }

    pub async fn run(&self) -> SimResult {
        let rx = take_option!(self.state.rx);
        let tx = take_option!(self.state.tx);

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
// ANCHOR_END: implFlaky

impl<T> State<T>
where
    T: SimObject,
{
    /// Return the next random u32
    ///
    /// This is wrapped in a separate function to hide the interior mutation
    fn next_u32(&self) -> u32 {
        self.rng.borrow_mut().next_u32()
    }
}
