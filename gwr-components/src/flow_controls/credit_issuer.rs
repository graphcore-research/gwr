// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Issue credit to a
//! [credit limiter](crate::flow_controls::credit_limiter)
//! for an output port.
//!
//! # Ports
//!
//! This component has the following ports:
//!  - One [input port](gwr_engine::port::InPort): `rx`
//!  - Two [output ports](gwr_engine::port::OutPort): `tx`, `credit_tx`

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::traits::{Runnable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::EntityDisplay;
use gwr_track::entity::Entity;
use gwr_track::trace;

use crate::types::Credit;
use crate::{connect_tx, port_rx, take_option};

#[derive(EntityDisplay)]
pub struct CreditIssuer<T>
where
    T: SimObject,
{
    pub entity: Rc<Entity>,
    tx: RefCell<Option<OutPort<T>>>,
    credit_tx: RefCell<Option<OutPort<Credit>>>,
    rx: RefCell<Option<InPort<T>>>,
}

impl<T> CreditIssuer<T>
where
    T: SimObject,
{
    pub fn new_and_register(engine: &Engine, parent: &Rc<Entity>) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, "credit_issue"));
        let tx = OutPort::new(&entity, "tx");
        let credit_tx = OutPort::new(&entity, "credit_tx");
        let rx = InPort::new(&entity, "rx");
        let rc_self = Rc::new(Self {
            entity,
            tx: RefCell::new(Some(tx)),
            credit_tx: RefCell::new(Some(credit_tx)),
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

    pub fn connect_port_credit_tx(&self, port_state: PortStateResult<Credit>) -> SimResult {
        connect_tx!(self.credit_tx, connect ; port_state)
    }
}

#[async_trait(?Send)]
impl<T> Runnable for CreditIssuer<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        let rx = take_option!(self.rx);
        let credit_tx = take_option!(self.credit_tx);
        let tx = take_option!(self.tx);

        loop {
            let value = rx.get()?.await;
            trace!(self.entity ; "issue credit");
            credit_tx.put(Credit(1))?.await;
            tx.put(value)?.await;
        }
    }
}
