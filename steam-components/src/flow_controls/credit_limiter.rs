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

use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortState};
use steam_engine::spawn_subcomponent;
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_resources::Resource;
use steam_track::entity::Entity;
use steam_track::trace;

use crate::types::Credit;
use crate::{connect_tx, port_rx, take_option};

struct CreditState {
    credit: Resource,
    rx: RefCell<Option<InPort<Credit>>>,
}

impl CreditState {
    fn new(entity: &Arc<Entity>, credit: Resource) -> Self {
        Self {
            credit,
            rx: RefCell::new(Some(InPort::new(entity, "rx"))),
        }
    }
}

#[derive(Clone, EntityDisplay)]
struct PortCredit {
    pub entity: Arc<Entity>,
    state: Rc<CreditState>,
}

impl PortCredit {
    pub fn new(parent: &Arc<Entity>, name: &str, credit: Resource) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        let state = Rc::new(CreditState::new(&entity, credit));
        Self { entity, state }
    }

    pub fn port_rx(&self) -> Rc<PortState<Credit>> {
        port_rx!(self.state.rx, state)
    }

    pub async fn run(&self) -> SimResult {
        let rx = take_option!(self.state.rx);
        let credit = self.state.credit.clone();

        loop {
            let credits = rx.get().await;
            for _ in 0..credits.0 {
                trace!(self.entity ; "release credit");
                credit.release().await?;
            }
        }
    }
}

struct CreditLimiterState<T>
where
    T: SimObject,
{
    credit: Resource,

    tx: RefCell<Option<OutPort<T>>>,
    credit_rx: RefCell<Option<PortCredit>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> CreditLimiterState<T>
where
    T: SimObject,
{
    fn new(entity: &Arc<Entity>, num_credits: usize) -> Self {
        let credit = Resource::new(num_credits);
        let credit_rx = PortCredit::new(entity, "credit_rx", credit.clone());

        Self {
            credit,
            tx: RefCell::new(Some(OutPort::new(entity, "tx"))),
            credit_rx: RefCell::new(Some(credit_rx)),
            rx: RefCell::new(Some(InPort::new(entity, "rx"))),
        }
    }
}

#[derive(Clone, EntityDisplay)]
pub struct CreditLimiter<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    state: Rc<CreditLimiterState<T>>,
}

impl<T> CreditLimiter<T>
where
    T: SimObject,
{
    pub fn new(parent: &Arc<Entity>, spawner: Spawner, num_credits: usize) -> Self {
        let entity = Arc::new(Entity::new(parent, "credit"));
        let state = Rc::new(CreditLimiterState::new(&entity, num_credits));

        Self {
            entity,
            state,
            spawner,
        }
    }

    pub fn connect_port_tx(&self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.tx, connect ; port_state);
    }

    pub fn port_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.rx, state)
    }

    pub fn port_credit_rx(&self) -> Rc<PortState<Credit>> {
        port_rx!(self.state.credit_rx, port_rx)
    }

    pub async fn run(&self) -> SimResult {
        let rx = take_option!(self.state.rx);
        let tx = take_option!(self.state.tx);
        let credit = self.state.credit.clone();

        spawn_subcomponent!(self.spawner ; self.state.credit_rx);

        loop {
            let value = rx.get().await;

            credit.request().await;
            trace!(self.entity ; "consume credit");

            tx.put(value).await?;
        }
    }
}
