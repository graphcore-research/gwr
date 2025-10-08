// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This component can be placed between two components in order to limit the
//! bandwidth between them.
//!
//! # Ports
//!
//! This component has the following ports:
//!  - One [input port](tramway_engine::port::InPort): `rx`
//!  - One [output port](tramway_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use tramway_engine::engine::Engine;
use tramway_engine::port::{InPort, OutPort, PortStateResult};
use tramway_engine::traits::{Runnable, SimObject};
use tramway_engine::types::{SimError, SimResult};
use tramway_model_builder::EntityDisplay;
use tramway_track::entity::Entity;
use tramway_track::{enter, exit};

use super::rate_limiter::RateLimiter;
use crate::{connect_tx, port_rx, take_option};

/// The [`Limiter`] is a component that will allow data through at a
/// specified rate.
///
/// The rate is defined in bits-per-second.
#[derive(EntityDisplay)]
pub struct Limiter<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    limiter: Rc<RateLimiter<T>>,
    tx: RefCell<Option<OutPort<T>>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> Limiter<T>
where
    T: SimObject,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        limiter: Rc<RateLimiter<T>>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));
        let tx = OutPort::new(&entity, "tx");
        let rx = InPort::new(&entity, "rx");
        let rc_self = Rc::new(Self {
            entity,
            limiter,
            tx: RefCell::new(Some(tx)),
            rx: RefCell::new(Some(rx)),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.tx, connect ; port_state)
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        port_rx!(self.rx, state)
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Limiter<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        let tx = take_option!(self.tx);
        let limiter = &self.limiter;
        loop {
            // Get the value but without letting the OutPort complete
            let value = rx.start_get()?.await;

            let value_id = value.id();
            let ticks = limiter.ticks(&value);
            enter!(self.entity ; value_id);

            tx.put(value)?.await;
            limiter.delay_ticks(ticks).await;
            exit!(self.entity ; value_id);

            // Allow the OutPort to complete
            rx.finish_get();
        }
    }
}
