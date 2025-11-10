// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A simple node of a ring.
//!
//! Includes routing of values out of the ring and arbitration of new values
//! verses values already on the ring.
//!
//! # Ports
//!
//! This component has four ports:
//!  - Two [input ports](gwr_engine::port::InPort): `ring_rx`, `io_rx`
//!  - Two [output ports](gwr_engine::port::OutPort): `ring_tx`, `io_tx`
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

use async_trait::async_trait;
use gwr_components::arbiter::{Arbiter, Arbitrate};
use gwr_components::connect_port;
use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::flow_controls::rate_limiter::RateLimiter;
use gwr_components::router::{Route, Router};
use gwr_components::store::Store;
use gwr_engine::engine::Engine;
use gwr_engine::port::PortStateResult;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Routable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet, Runnable};
use gwr_track::build_aka;
use gwr_track::entity::{Entity, GetEntity};
use gwr_track::tracker::aka::Aka;

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

#[derive(EntityGet, EntityDisplay, Runnable)]
pub struct RingNode<T>
where
    T: SimObject + Routable,
{
    entity: Rc<Entity>,
    rx_buffer_limiter: Rc<Limiter<T>>,
    tx_buffer: Rc<Store<T>>,
    arbiter: Rc<Arbiter<T>>,
    router: Rc<Router<T>>,
}

impl<T> RingNode<T>
where
    T: SimObject + Routable,
{
    #[expect(clippy::too_many_arguments)]
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: &RingConfig<T>,
        routing_algorithm: Box<dyn Route<T>>,
        policy: Box<dyn Arbitrate<T>>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));

        let rx_buffer_limiter_aka = build_aka!(aka, &entity, &[("ring_rx", "rx")]);
        let rx_buffer_limiter = Limiter::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "limit_rx",
            Some(&rx_buffer_limiter_aka),
            config.write_limiter.clone(),
        )?;
        let rx_buffer =
            Store::new_and_register(engine, clock, &entity, "rx_buf", config.rx_buffer_entries)?;
        connect_port!(rx_buffer_limiter, tx => rx_buffer, rx)?;

        let tx_buffer_limiter = Limiter::new_and_register(
            engine,
            clock,
            &entity,
            "limit_tx",
            config.write_limiter.clone(),
        )?;
        let tx_buffer_aka = build_aka!(aka, &entity, &[("ring_tx", "tx")]);
        let tx_buffer = Store::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "tx_buf",
            Some(&tx_buffer_aka),
            config.tx_buffer_entries,
        )?;
        connect_port!(tx_buffer_limiter, tx => tx_buffer, rx)?;

        let router_aka = build_aka!(aka, &entity, &[("io_tx", &format!("tx_{IO_INDEX}"))]);
        let router = Router::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "router",
            Some(&router_aka),
            2,
            routing_algorithm,
        )?;
        connect_port!(rx_buffer, tx => router, rx)?;

        let arbiter_aka = build_aka!(aka, &entity, &[("io_rx", &format!("rx_{IO_INDEX}"))]);
        let arbiter = Arbiter::new_and_register_with_renames(
            engine,
            clock,
            &entity,
            "arb",
            Some(&arbiter_aka),
            2,
            policy,
        )?;
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

    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: &RingConfig<T>,
        routing_algorithm: Box<dyn Route<T>>,
        policy: Box<dyn Arbitrate<T>>,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(
            engine,
            clock,
            parent,
            name,
            None,
            config,
            routing_algorithm,
            policy,
        )
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
