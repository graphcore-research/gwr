// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Perform routing between an input interface and a number number of outputs.
//!
//! The [Router] is passed an algorithm that makes the decision about which
//! egress port to send each routed object to.
//!
//! # Ports
//!
//! This component has the following ports:
//!  - One [input port](gwr_engine::port::InPort): `rx`
//!  - N [output ports](gwr_engine::port::OutPort): `tx[i]` for `i in [0, N-1]`

//! # Function
//!
//! The [Router] will take objects from the single input and send them to the
//! correct output. A simplified summary of its functionality is:
//!
//! ```rust
//! # use std::rc::Rc;
//! # use async_trait::async_trait;
//! # use gwr_components::router::Route;
//! # use gwr_engine::port::{InPort, OutPort};
//! # use gwr_engine::sim_error;
//! # use gwr_engine::time::clock::Clock;
//! # use gwr_engine::traits::{Routable, SimObject};
//! # use gwr_engine::types::SimResult;
//! # use gwr_track::entity::Entity;
//! #
//! # async fn run<T>(
//! #     tx: Vec<OutPort<T>>,
//! #     rx: InPort<T>,
//! #     routing_algorithm: Box<dyn Route<T>>
//! # ) -> SimResult
//! # where
//! #     T: SimObject + Routable
//! # {
//! loop {
//!     let value = rx.get()?.await;
//!     let tx_index = routing_algorithm.route(&value)?;
//!
//!     match tx.get(tx_index) {
//!         None => {
//!             // Report error
//!         }
//!         Some(tx) => {
//!             tx.put(value)?.await;
//!         }
//!     }
//! }
//! # }
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Routable, Runnable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::EntityDisplay;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;
use gwr_track::{enter, exit, trace};

use crate::take_option;

/// Trait required for routing algorithms to implement.
pub trait Route<T>
where
    T: Routable,
{
    /// Given an object, return the index of the egress port to map the object
    /// to.
    fn route(&self, object: &T) -> Result<usize, SimError>;
}

pub struct DefaultAlgorithm {}

impl<T> Route<T> for DefaultAlgorithm
where
    T: Routable,
{
    /// Determine route by taking the object destination as an index.
    fn route(&self, obj_to_route: &T) -> Result<usize, SimError> {
        Ok(obj_to_route.destination() as usize)
    }
}

#[derive(EntityDisplay)]
pub struct Router<T>
where
    T: SimObject + Routable,
{
    pub entity: Rc<Entity>,
    rx: RefCell<Option<InPort<T>>>,
    tx: RefCell<Vec<OutPort<T>>>,
    algorithm: Box<dyn Route<T>>,
}

impl<T> Router<T>
where
    T: SimObject + Routable,
{
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        num_egress: usize,
        algorithm: Box<dyn Route<T>>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);
        let mut tx = Vec::with_capacity(num_egress);
        for i in 0..num_egress {
            tx.push(OutPort::new_with_renames(&entity, &format!("tx_{i}"), aka));
        }
        let rc_self = Rc::new(Self {
            entity,
            rx: RefCell::new(Some(rx)),
            tx: RefCell::new(tx),
            algorithm,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        num_egress: usize,
        algorithm: Box<dyn Route<T>>,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(
            engine, clock, parent, name, None, num_egress, algorithm,
        )
    }

    pub fn connect_port_tx_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult {
        match self.tx.borrow_mut().get_mut(i) {
            None => {
                sim_error!(format!("{self}: no tx port {i}"))
            }
            Some(tx) => tx.connect(port_state),
        }
    }

    pub fn port_rx(&self) -> PortStateResult<T> {
        self.rx.borrow().as_ref().unwrap().state()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Router<T>
where
    T: SimObject + Routable,
{
    async fn run(&self) -> SimResult {
        let tx: Vec<OutPort<T>> = self.tx.borrow_mut().drain(..).collect();
        let rx = take_option!(self.rx);
        let algorithm = &self.algorithm;

        loop {
            let value = rx.get()?.await;
            enter!(self.entity ; value.id());

            let tx_index = algorithm.route(&value)?;
            trace!(self.entity ; "Route {} to {}", value, tx_index);

            match tx.get(tx_index) {
                None => {
                    return sim_error!(format!(
                        "{self}: {value:?} selected invalid egress index {tx_index}"
                    ));
                }
                Some(tx) => {
                    exit!(self.entity ; value.id());
                    tx.put(value)?.await;
                }
            }
        }
    }
}
