// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Perform routing between an input interface and a number number of outputs.
//!
//! # Ports
//!
//! This component has one input and `N`-output ports:
//!  - One [input port](steam_engine::port::InPort): `rx`
//!  - N [output ports](steam_engine::port::OutPort): `tx[i]` for `i in [0,
//!    N-1]`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_engine::engine::Engine;
use steam_engine::port::{InPort, OutPort, PortState};
use steam_engine::traits::{Routable, Runnable, SimObject};
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::{enter, exit, trace};

use crate::take_option;

pub trait Route<T>
where
    T: Routable,
{
    fn route(&self, object: &T) -> usize;
}

pub struct DefaultRouter {}

impl<T> Route<T> for DefaultRouter
where
    T: Routable,
{
    fn route(&self, obj_to_route: &T) -> usize {
        obj_to_route.dest() as usize
    }
}

#[derive(EntityDisplay)]
pub struct Router<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    rx: RefCell<Option<InPort<T>>>,
    tx: RefCell<Vec<OutPort<T>>>,
    router: Box<dyn Route<T>>,
}

impl<T> Router<T>
where
    T: SimObject,
{
    #[must_use]
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        num_egress: usize,
        router: Box<dyn Route<T>>,
    ) -> Rc<Self> {
        let entity = Arc::new(Entity::new(parent, name));
        let rx = InPort::new(&entity, "rx");
        let mut tx = Vec::with_capacity(num_egress);
        for i in 0..num_egress {
            tx.push(OutPort::new(&entity, format!("tx{i}").as_str()));
        }
        let rc_self = Rc::new(Self {
            entity,
            rx: RefCell::new(Some(rx)),
            tx: RefCell::new(tx),
            router,
        });
        engine.register(rc_self.clone());
        rc_self
    }

    pub fn connect_port_tx_i(&self, i: usize, port_state: Rc<PortState<T>>) {
        match self.tx.borrow_mut().get_mut(i) {
            None => {
                panic!("{}: no tx port {}", self.entity, i);
            }
            Some(tx) => tx.connect(port_state),
        }
    }

    #[must_use]
    pub fn port_rx(&self) -> Rc<PortState<T>> {
        self.rx.borrow().as_ref().unwrap().state()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Router<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        let tx: Vec<OutPort<T>> = self.tx.borrow_mut().drain(..).collect();
        let rx = take_option!(self.rx);
        let router = &self.router;

        loop {
            let value = rx.get().await;
            enter!(self.entity ; value.tag());

            let tx_index = router.route(&value);
            trace!(self.entity ; "Route {} to {}", value, tx_index);

            match tx.get(tx_index) {
                None => {
                    panic!(
                        "{}: {:?} selected invalid egress index {}",
                        self.entity, value, tx_index
                    );
                }
                Some(tx) => {
                    exit!(self.entity ; value.tag());
                    tx.put(value).await?;
                }
            }
        }
    }
}
