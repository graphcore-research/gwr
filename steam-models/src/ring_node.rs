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
//! ```text
//!    +-------------------------------------------------------------+
//!    |            Rx                             Tx                |
//! -> | ring_rx -> buffer -> router -> arbiter -> buffer -> ring_tx | ->
//!    |                        |          ^                         |
//!    |                        \----------|-----------------> io_tx | ->
//! -> | io_rx ----------------------------/                         |
//!    +-------------------------------------------------------------+
//! ```

use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_components::arbiter::{Arbiter, Arbitrate};
use steam_components::connect_port;
use steam_components::flow_controls::limiter::Limiter;
use steam_components::flow_controls::rate_limiter::RateLimiter;
use steam_components::router::{Route, Router};
use steam_components::store::Store;
use steam_engine::engine::Engine;
use steam_engine::executor::Spawner;
use steam_engine::port::PortStateResult;
use steam_engine::traits::{Routable, SimObject};
use steam_engine::types::{SimError, SimResult};
use steam_model_builder::{EntityDisplay, Runnable};
use steam_track::entity::Entity;

/// The port index used for ring connections.
pub const RING_INDEX: usize = 0;
/// The port index used for I/O connections.
pub const IO_INDEX: usize = 1;

pub struct RingConfig<T>
where
    T: SimObject,
{
    rx_buffer_entries: usize,
    tx_buffer_entries: usize,
    write_limiter: Rc<RateLimiter<T>>,
}

impl<T> RingConfig<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new(
        rx_buffer_entries: usize,
        tx_buffer_entries: usize,
        write_limiter: Rc<RateLimiter<T>>,
    ) -> Self {
        Self {
            rx_buffer_entries,
            tx_buffer_entries,
            write_limiter,
        }
    }
}

#[derive(EntityDisplay, Runnable)]
pub struct RingNode<T>
where
    T: SimObject + Routable,
{
    pub entity: Arc<Entity>,
    rx_buffer_limiter: Rc<Limiter<T>>,
    tx_buffer: Rc<Store<T>>,
    arbiter: Rc<Arbiter<T>>,
    router: Rc<Router<T>>,
}

impl<T> RingNode<T>
where
    T: SimObject + Routable,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        spawner: Spawner,
        config: &RingConfig<T>,
        route_fn: Box<dyn Route<T>>,
        policy: Box<dyn Arbitrate<T>>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));

        let rx_buffer_limiter =
            Limiter::new_and_register(engine, &entity, "limit_rx", config.write_limiter.clone())?;
        let rx_buffer = Store::new_and_register(
            engine,
            &entity,
            "rx_buf",
            spawner.clone(),
            config.rx_buffer_entries,
        )?;
        connect_port!(rx_buffer_limiter, tx => rx_buffer, rx)?;

        let tx_buffer_limiter =
            Limiter::new_and_register(engine, &entity, "limit_tx", config.write_limiter.clone())?;
        let tx_buffer = Store::new_and_register(
            engine,
            &entity,
            "tx_buf",
            spawner.clone(),
            config.tx_buffer_entries,
        )?;
        connect_port!(tx_buffer_limiter, tx => tx_buffer, rx)?;

        let router = Router::new_and_register(engine, &entity, "router", 2, route_fn)?;
        connect_port!(rx_buffer, tx => router, rx)?;

        let arbiter = Arbiter::new_and_register(engine, &entity, "arb", spawner, 2, policy)?;
        connect_port!(router, tx, RING_INDEX => arbiter, rx, RING_INDEX)?;
        connect_port!(arbiter, tx => tx_buffer_limiter, rx)?;

        let rc_self = Rc::new(Self {
            entity,
            rx_buffer_limiter,
            tx_buffer,
            arbiter,
            router,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn connect_port_ring_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        self.tx_buffer.connect_port_tx(port_state)
    }

    pub fn connect_port_io_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        self.router.connect_port_tx_i(IO_INDEX, port_state)
    }

    pub fn port_ring_rx(&self) -> PortStateResult<T> {
        self.rx_buffer_limiter.port_rx()
    }

    pub fn port_io_rx(&self) -> PortStateResult<T> {
        self.arbiter.port_rx_i(IO_INDEX)
    }
}
