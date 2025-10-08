// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Perform arbitration between a number of interfaces.
//!
//! # Ports
//!
//! This component has the following ports:
//!  - N [input ports](tramway_engine::port::InPort): `rx[i]` for `i in [0,
//!    N-1]`
//!  - One [output port](tramway_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use tramway_engine::engine::Engine;
use tramway_engine::events::once::Once;
use tramway_engine::executor::Spawner;
use tramway_engine::port::{InPort, OutPort, PortStateResult};
use tramway_engine::traits::{Event, Runnable, SimObject};
use tramway_engine::types::{SimError, SimResult};
use tramway_model_builder::EntityDisplay;
use tramway_track::entity::Entity;
use tramway_track::{enter, exit, trace};

use crate::{connect_tx, take_option};

pub mod policy;

#[derive(Default)]
struct ArbiterSharedState<T> {
    input_values: RefCell<Vec<Option<T>>>,
    arbiter_event: RefCell<Option<Once<()>>>,
    waiting_put: Vec<RefCell<Option<Once<()>>>>,
}

impl<T> ArbiterSharedState<T> {
    fn new(capacity: usize) -> Self {
        Self {
            input_values: RefCell::new((0..capacity).map(|_| None).collect()),
            arbiter_event: RefCell::new(None),
            waiting_put: (0..capacity).map(|_| RefCell::new(None)).collect(),
        }
    }
}

pub trait Arbitrate<T>
where
    T: SimObject,
{
    fn arbitrate(
        &mut self,
        entity: &Arc<Entity>,
        input_values: &mut [Option<T>],
    ) -> Option<(usize, T)>;
}

#[derive(EntityDisplay)]
pub struct Arbiter<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    rx: RefCell<Vec<Option<InPort<T>>>>,
    tx: RefCell<Option<OutPort<T>>>,
    policy: RefCell<Option<Box<dyn Arbitrate<T>>>>,
    shared_state: Rc<ArbiterSharedState<T>>,
    spawner: Spawner,
}

impl<T> Arbiter<T>
where
    T: SimObject,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        spawner: Spawner,
        num_rx: usize,
        policy: Box<dyn Arbitrate<T>>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));
        let shared_state = Rc::new(ArbiterSharedState::new(num_rx));
        let rx = (0..num_rx)
            .map(|i| Some(InPort::new(&entity, format!("rx{i}").as_str())))
            .collect();
        let tx = OutPort::new(&entity, "tx");
        let rc_self = Rc::new(Self {
            entity,
            rx: RefCell::new(rx),
            tx: RefCell::new(Some(tx)),
            policy: RefCell::new(Some(policy)),
            shared_state,
            spawner,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn connect_port_tx(&self, port_state: PortStateResult<T>) -> SimResult {
        connect_tx!(self.tx, connect ; port_state)
    }

    pub fn port_rx_i(&self, i: usize) -> PortStateResult<T> {
        self.rx.borrow()[i].as_ref().unwrap().state()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Arbiter<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        // Start running the handlers for each input
        for (i, mut rx) in self.rx.borrow_mut().drain(..).enumerate() {
            let entity = self.entity.clone();
            let rx = rx.take().unwrap();
            let shared_state = self.shared_state.clone();
            self.spawner
                .spawn(async move { run_input(entity, rx, i, shared_state).await });
        }

        let tx = take_option!(self.tx);
        let mut policy = take_option!(self.policy);

        // Drive the output
        loop {
            let wait_event;
            loop {
                let value;
                let wake_event;
                {
                    let mut input_values = self.shared_state.input_values.borrow_mut();
                    let t = policy.arbitrate(&self.entity, &mut input_values);
                    match t {
                        Some((i, t)) => {
                            trace!(self.entity ; "grant {}: {}", i, t);
                            wake_event = self.shared_state.waiting_put[i].borrow_mut().take();
                            value = t;
                        }
                        None => {
                            wait_event = Once::default();
                            trace!(self.entity ; "arb wait");
                            *self.shared_state.arbiter_event.borrow_mut() =
                                Some(wait_event.clone());
                            break;
                        }
                    }
                }

                if let Some(event) = wake_event {
                    event.notify()?;
                }
                exit!(self.entity ; value.id());
                tx.put(value)?.await;
            }
            wait_event.listen().await;
        }
    }
}

async fn run_input<T: SimObject>(
    entity: Arc<Entity>,
    rx: InPort<T>,
    input_idx: usize,
    shared_state: Rc<ArbiterSharedState<T>>,
) -> SimResult {
    loop {
        let value = rx.get()?.await;
        enter!(entity ; value.id());

        // Check if this input needs to wait for the previous value to be handled
        let wait_for_space = match shared_state.input_values.borrow()[input_idx].as_ref() {
            Some(_) => {
                let wait_for_space = Once::default();
                *shared_state.waiting_put[input_idx].borrow_mut() = Some(wait_for_space.clone());
                Some(wait_for_space)
            }
            None => None,
        };
        if let Some(wait_event) = wait_for_space {
            wait_event.listen().await;
        }

        // Set the value for this input
        shared_state.input_values.borrow_mut()[input_idx] = Some(value);

        // Wake up the arbiter if it has paused on an event
        if let Some(arbiter_event) = shared_state.arbiter_event.borrow_mut().take() {
            arbiter_event.notify().unwrap();
        }
    }
}
