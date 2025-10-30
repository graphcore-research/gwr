// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Enforce credit limit on an interface between two ports.
//!
//! # Ports
//!
//! This component has the following ports:
//!  - Two [input ports](gwr_engine::port::InPort): `rx`, `credit_rx`
//!  - One [output port](gwr_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::spawn_subcomponent;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Runnable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::EntityDisplay;
use gwr_resources::Resource;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;
use gwr_track::{build_aka, trace};

use crate::types::Credit;
use crate::{connect_tx, port_rx, take_option};

#[derive(EntityDisplay)]
struct PortCredit {
    pub entity: Rc<Entity>,
    credit: Resource,
    rx: RefCell<Option<InPort<Credit>>>,
}

impl PortCredit {
    pub fn new(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        credit: Resource,
    ) -> Self {
        let entity = Rc::new(Entity::new(parent, name));
        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);
        Self {
            entity,
            credit,
            rx: RefCell::new(Some(rx)),
        }
    }

    pub fn port_rx(&self) -> PortStateResult<Credit> {
        port_rx!(self.rx, state)
    }

    pub async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        let credit = self.credit.clone();

        loop {
            let credits = rx.get()?.await;
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
    pub entity: Rc<Entity>,
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
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        num_credits: usize,
    ) -> Result<Rc<Self>, SimError> {
        let spawner = engine.spawner();
        let entity = Rc::new(Entity::new(parent, name));
        let credit = Resource::new(num_credits);

        let credit_rx_aka = build_aka!(aka, &entity, &[("credit_rx", "rx")]);
        let credit_rx: PortCredit = PortCredit::new(
            engine,
            clock,
            &entity,
            "credit_rx",
            Some(&credit_rx_aka),
            credit.clone(),
        );
        let tx = OutPort::new_with_renames(&entity, "tx", aka);
        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);

        let rc_self = Rc::new(Self {
            entity,
            credit,
            tx: RefCell::new(Some(tx)),
            credit_rx: RefCell::new(Some(credit_rx)),
            rx: RefCell::new(Some(rx)),
            spawner,
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

    pub fn port_credit_rx(&self) -> PortStateResult<Credit> {
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
            let value = rx.get()?.await;

            credit.request().await;
            trace!(self.entity ; "consume credit");

            tx.put(value)?.await;
        }
    }
}
