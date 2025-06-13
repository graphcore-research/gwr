// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This component can be placed between two components in order to limit the
//! bandwidth between them.
//!
//! # Ports
//!
//! This component has two ports
//!  - One [input port](steam_engine::port::InPort): `rx`
//!  - One [output port](steam_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use steam_engine::port::{InPort, OutPort, PortState};
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::{enter, exit};

use super::rate_limiter::RateLimiter;
use crate::{connect_tx, port_rx, take_option};

struct LimiterState<T>
where
    T: SimObject,
{
    limiter: Rc<RateLimiter<T>>,
    tx: RefCell<Option<OutPort<T>>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> LimiterState<T>
where
    T: SimObject,
{
    fn new(entity: &Arc<Entity>, limiter: Rc<RateLimiter<T>>) -> Self {
        Self {
            limiter,
            tx: RefCell::new(Some(OutPort::new(entity, "tx"))),
            rx: RefCell::new(Some(InPort::new(entity, "rx"))),
        }
    }
}

/// The [`Limiter`] is a component that will allow data through at a
/// specified rate.
///
/// The rate is defined in bits-per-second.
#[derive(Clone, EntityDisplay)]
pub struct Limiter<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    state: Rc<LimiterState<T>>,
}

impl<T> Limiter<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new(parent: &Arc<Entity>, name: &str, limiter: Rc<RateLimiter<T>>) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        let state = Rc::new(LimiterState::new(&entity, limiter));
        Self { entity, state }
    }

    pub fn connect_port_tx(&self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.tx, connect ; port_state);
    }

    #[must_use]
    pub fn port_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.rx, state)
    }

    pub async fn run(&self) -> SimResult {
        let rx = take_option!(self.state.rx);
        let tx = take_option!(self.state.tx);
        let limiter = &self.state.limiter;
        loop {
            // Get the value but without letting the OutPort complete
            let value = rx.start_get().await;

            let value_tag = value.tag();
            let ticks = limiter.ticks(&value);
            enter!(self.entity ; value_tag);

            tx.put(value).await?;
            limiter.delay_ticks(ticks).await;
            exit!(self.entity ; value_tag);

            // Allow the OutPort to complete
            rx.finish_get();
        }
    }
}
