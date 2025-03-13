// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Issue credit to a
//! [credit limiter](crate::flow_controls::credit_limiter)
//! for an output port.
//!
//! # Ports
//!
//! This component has three ports:
//!  - One [input port](steam_engine::port::InPort): `rx`
//!  - Two [output ports](steam_engine::port::OutPort): `tx`, `credit_tx`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use steam_engine::port::OutPort;
use steam_engine::port::{InPort, PortState};
use steam_engine::traits::SimObject;
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::trace;

use crate::types::Credit;
use crate::{connect_tx, port_rx, take_option};

struct CreditIssuerState<T>
where
    T: SimObject,
{
    tx: RefCell<Option<OutPort<T>>>,
    credit_tx: RefCell<Option<OutPort<Credit>>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> CreditIssuerState<T>
where
    T: SimObject,
{
    fn new(entity: Arc<Entity>) -> Self {
        Self {
            tx: RefCell::new(Some(OutPort::new(entity.clone(), "tx"))),
            credit_tx: RefCell::new(Some(OutPort::new(entity.clone(), "credit_tx"))),
            rx: RefCell::new(Some(InPort::new(entity))),
        }
    }
}

#[derive(Clone, EntityDisplay)]
pub struct CreditIssuer<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    state: Rc<CreditIssuerState<T>>,
}

impl<T> CreditIssuer<T>
where
    T: SimObject,
{
    pub fn new(parent: &Arc<Entity>) -> Self {
        let entity = Arc::new(Entity::new(parent, "credit_issue"));
        Self {
            entity: entity.clone(),
            state: Rc::new(CreditIssuerState::new(entity)),
        }
    }

    pub fn connect_port_tx(&self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.tx, connect ; port_state);
    }

    pub fn port_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.rx, state)
    }

    pub fn connect_port_credit_tx(&self, port_state: Rc<PortState<Credit>>) {
        connect_tx!(self.state.credit_tx, connect ; port_state);
    }

    pub async fn run(&self) -> SimResult {
        let rx = take_option!(self.state.rx);
        let credit_tx = take_option!(self.state.credit_tx);
        let tx = take_option!(self.state.tx);

        loop {
            let value = rx.get().await;
            trace!(self.entity ; "issue credit");
            credit_tx.put(Credit(1)).await?;
            tx.put(value).await?;
        }
    }
}
