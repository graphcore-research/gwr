// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Enforce credit limit on an interface between two ports.
//!
//! # Ports
//!
//! This component has three ports:
//!  - Two [input ports](steam_engine::port::InPort): `rx`, `credit_rx`
//!  - One [output port](steam_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_engine::engine::Engine;
use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortState};
use steam_engine::spawn_subcomponent;
use steam_engine::traits::{Runnable, SimObject};
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_resources::Resource;
use steam_track::entity::Entity;
use steam_track::trace;

use crate::types::Credit;
use crate::{connect_tx, port_rx, take_option};

#[derive(EntityDisplay)]
struct PortCredit {
    pub entity: Arc<Entity>,
    credit: Resource,
    rx: RefCell<Option<InPort<Credit>>>,
}

impl PortCredit {
    pub fn new(parent: &Arc<Entity>, name: &str, credit: Resource) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        let rx = InPort::new(&entity, "rx");
        Self {
            entity,
            credit,
            rx: RefCell::new(Some(rx)),
        }
    }

    pub fn port_rx(&self) -> Rc<PortState<Credit>> {
        port_rx!(self.rx, state)
    }

    pub async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        let credit = self.credit.clone();

        loop {
            let credits = rx.get().await;
            for _ in 0..credits.0 {
                trace!(self.entity ; "release credit");
                credit.release().await?;
            }
        }
    }
}

#[derive(EntityDisplay)]
pub struct CreditLimiter<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    credit: Resource,

    tx: RefCell<Option<OutPort<T>>>,
    credit_rx: RefCell<Option<PortCredit>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> CreditLimiter<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        spawner: Spawner,
        num_credits: usize,
    ) -> Rc<Self> {
        let entity = Arc::new(Entity::new(parent, "credit"));
        let credit = Resource::new(num_credits);
        let credit_rx = PortCredit::new(&entity, "credit_rx", credit.clone());
        let tx = OutPort::new(&entity, "tx");
        let rx = InPort::new(&entity, "rx");

        let rc_self = Rc::new(Self {
            entity,
            credit,
            tx: RefCell::new(Some(tx)),
            credit_rx: RefCell::new(Some(credit_rx)),
            rx: RefCell::new(Some(rx)),
            spawner,
        });
        engine.register(rc_self.clone());
        rc_self
    }

    pub fn connect_port_tx(&self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.tx, connect ; port_state);
    }

    #[must_use]
    pub fn port_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.rx, state)
    }

    #[must_use]
    pub fn port_credit_rx(&self) -> Rc<PortState<Credit>> {
        port_rx!(self.credit_rx, port_rx)
    }
}

#[async_trait(?Send)]
impl<T> Runnable for CreditLimiter<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        let tx = take_option!(self.tx);
        let credit = self.credit.clone();

        spawn_subcomponent!(self.spawner ; self.credit_rx);

        loop {
            let value = rx.get().await;

            credit.request().await;
            trace!(self.entity ; "consume credit");

            tx.put(value).await?;
        }
    }
}
