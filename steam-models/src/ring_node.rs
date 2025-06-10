// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A simple node of a ring.
//!
//! Includes routing of values out of the ring and arbitration of new values
//! verses values already on the ring.
//!
//! # Ports
//!
//! This component has four ports:
//!  - Two [input ports](steam_engine::port::InPort): `rx[RING]`, `rx[IO]`
//!  - Two [output ports](steam_engine::port::OutPort): `tx[RING]`, `tx[IO]`
//!
//! # Diagram
//!
//! ```rust
//! //    +-------------------------------------------------------------+
//! //    |            Rx                             Tx                |
//! // -> | ring_rx -> buffer -> router -> arbiter -> buffer -> ring_tx | ->
//! //    |                        |          ^                         |
//! //    |                        \----------|-----------------> io_tx | ->
//! // -> | io_rx ----------------------------/                         |
//! //    +-------------------------------------------------------------+
//! # use std; // Just here to prevent doc warning for an invalid doc block
//! ```

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use steam_components::arbiter::{Arbiter, Arbitrate};
use steam_components::flow_controls::limiter::Limiter;
use steam_components::flow_controls::rate_limiter::RateLimiter;
use steam_components::router::{Route, Router};
use steam_components::store::Store;
use steam_components::{connect_port, connect_tx, connect_tx_i, port_rx, port_rx_i};
use steam_engine::executor::Spawner;
use steam_engine::port::PortState;
use steam_engine::spawn_subcomponent;
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;

/// The port index used for ring connections.
pub const RING_INDEX: usize = 0;
/// The port index used for I/O connections.
pub const IO_INDEX: usize = 1;

pub struct RingConfig {
    rx_buffer_entries: usize,
    tx_buffer_entries: usize,
}

impl RingConfig {
    pub fn new(rx_buffer_entries: usize, tx_buffer_entries: usize) -> Self {
        Self {
            rx_buffer_entries,
            tx_buffer_entries,
        }
    }
}

struct RingNodeState<T>
where
    T: SimObject,
{
    rx_buffer_limiter: RefCell<Option<Limiter<T>>>,
    rx_buffer: RefCell<Option<Store<T>>>,
    tx_buffer_limiter: RefCell<Option<Limiter<T>>>,
    tx_buffer: RefCell<Option<Store<T>>>,
    arbiter: RefCell<Option<Arbiter<T>>>,
    router: RefCell<Option<Router<T>>>,
}

impl<T> RingNodeState<T>
where
    T: SimObject,
{
    fn new(
        entity: &Arc<Entity>,
        spawner: Spawner,
        config: &RingConfig,
        write_limiter: Rc<RateLimiter<T>>,
        route_fn: Box<dyn Route<T>>,
        policy: Box<dyn Arbitrate<T>>,
    ) -> Self {
        let rx_buffer_limiter = Limiter::new(entity, "limit_rx", write_limiter.clone());
        let rx_buffer = Store::new(entity, "rx_buf", spawner.clone(), config.rx_buffer_entries);
        connect_port!(rx_buffer_limiter, tx => rx_buffer, rx);

        let tx_buffer_limiter = Limiter::new(entity, "limit_tx", write_limiter);
        let tx_buffer = Store::new(entity, "tx_buf", spawner.clone(), config.tx_buffer_entries);
        connect_port!(tx_buffer_limiter, tx => tx_buffer, rx);

        let router = Router::new(entity, "router", 2, route_fn);
        connect_port!(rx_buffer, tx => router, rx);

        let mut arbiter = Arbiter::new(entity, "arb", spawner, 2, policy);
        connect_port!(router, tx, RING_INDEX => arbiter, rx, RING_INDEX);
        connect_port!(arbiter, tx => tx_buffer_limiter, rx);

        Self {
            rx_buffer_limiter: RefCell::new(Some(rx_buffer_limiter)),
            rx_buffer: RefCell::new(Some(rx_buffer)),
            tx_buffer_limiter: RefCell::new(Some(tx_buffer_limiter)),
            tx_buffer: RefCell::new(Some(tx_buffer)),
            arbiter: RefCell::new(Some(arbiter)),
            router: RefCell::new(Some(router)),
        }
    }
}

#[derive(Clone, EntityDisplay)]
pub struct RingNode<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    state: Rc<RingNodeState<T>>,
}

impl<T> RingNode<T>
where
    T: SimObject,
{
    pub fn new(
        parent: &Arc<Entity>,
        name: &str,
        spawner: Spawner,
        config: &RingConfig,
        write_limiter: Rc<RateLimiter<T>>,
        route_fn: Box<dyn Route<T>>,
        policy: Box<dyn Arbitrate<T>>,
    ) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        let state = RingNodeState::new(
            &entity,
            spawner.clone(),
            config,
            write_limiter,
            route_fn,
            policy,
        );
        Self {
            entity,
            spawner,
            state: Rc::new(state),
        }
    }

    pub fn connect_port_ring_tx(&mut self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.tx_buffer, connect_port_tx ; port_state);
    }

    pub fn connect_port_io_tx(&mut self, port_state: Rc<PortState<T>>) {
        connect_tx_i!(self.state.router, connect_port_tx_i, IO_INDEX ; port_state);
    }

    pub fn port_ring_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.rx_buffer_limiter, port_rx)
    }

    pub fn port_io_rx(&self) -> Rc<PortState<T>> {
        port_rx_i!(self.state.arbiter, port_rx_i, IO_INDEX)
    }

    pub async fn run(&self) -> SimResult {
        spawn_subcomponent!(self.spawner ; self.state.rx_buffer_limiter);
        spawn_subcomponent!(self.spawner ; self.state.rx_buffer);
        spawn_subcomponent!(self.spawner ; self.state.tx_buffer_limiter);
        spawn_subcomponent!(self.spawner ; self.state.tx_buffer);
        spawn_subcomponent!(self.spawner ; self.state.arbiter);
        spawn_subcomponent!(self.spawner ; self.state.router);
        Ok(())
    }
}
