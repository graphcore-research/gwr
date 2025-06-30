// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Perform arbitration between a number of interfaces.
//!
//! # Ports
//!
//! This component has `N`-input ports and one output:
//!  - N [input ports](steam_engine::port::InPort): `rx[i]` for `i in [0, N-1]`
//!  - One [output port](steam_engine::port::OutPort): `tx`

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use steam_engine::engine::Engine;
use steam_engine::events::once::Once;
use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortStateResult};
use steam_engine::sim_error;
use steam_engine::traits::{Event, Runnable, SimObject};
use steam_engine::types::{SimError, SimResult};
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::{enter, exit, trace};

use crate::{connect_tx, take_option};

#[derive(Default)]
struct ArbiterSharedState<T> {
    active: RefCell<Vec<Option<T>>>,
    arbiter_event: RefCell<Option<Once<()>>>,
    waiting_put: Vec<RefCell<Option<Once<()>>>>,
}

impl<T> ArbiterSharedState<T> {
    fn new(capacity: usize) -> Self {
        Self {
            active: RefCell::new((0..capacity).map(|_| None).collect()),
            arbiter_event: RefCell::new(None),
            waiting_put: (0..capacity).map(|_| RefCell::new(None)).collect(),
        }
    }
}

pub trait Arbitrate<T>
where
    T: SimObject,
{
    fn arbitrate(&mut self, entity: &Arc<Entity>, inputs: &mut [Option<T>]) -> Option<(usize, T)>;
}

pub struct RoundRobinPolicy {
    candidate: usize,
}

impl RoundRobinPolicy {
    #[must_use]
    pub fn new() -> Self {
        Self { candidate: 0 }
    }
}

impl Default for RoundRobinPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Arbitrate<T> for RoundRobinPolicy
where
    T: SimObject,
{
    fn arbitrate(&mut self, _entity: &Arc<Entity>, inputs: &mut [Option<T>]) -> Option<(usize, T)> {
        let num_inputs = inputs.len();
        for i in 0..num_inputs {
            let index = (i + self.candidate) % num_inputs;
            if let Some(value) = inputs[index].take() {
                self.candidate = (index + 1) % num_inputs;
                return Some((index, value));
            }
        }
        None
    }
}

pub struct WeightedRoundRobinPolicy {
    candidate: usize,
    grants: Vec<usize>,
    weights: Vec<usize>,
}

impl WeightedRoundRobinPolicy {
    pub fn new(weights: Vec<usize>, num_inputs: usize) -> Result<Self, SimError> {
        if weights.len() != num_inputs {
            return sim_error!("The number of weights must be equal to the number of inputs");
        }

        Ok(Self {
            candidate: 0,
            grants: vec![0; weights.len()],
            weights,
        })
    }
}

impl WeightedRoundRobinPolicy {
    pub fn state_str<T>(&self, inputs: &[Option<T>]) -> String
    where
        T: SimObject,
    {
        let mut s = String::new();
        s.push_str(format!("{}: ", self.candidate).as_str());
        for (i, grant) in self.grants.iter().enumerate() {
            let req = if inputs[i].is_some() { "r" } else { "-" };
            s.push_str(format!("{}/{}/{}, ", req, grant, self.weights[i]).as_str());
        }
        s
    }
}

impl<T> Arbitrate<T> for WeightedRoundRobinPolicy
where
    T: SimObject,
{
    fn arbitrate(&mut self, entity: &Arc<Entity>, inputs: &mut [Option<T>]) -> Option<(usize, T)> {
        trace!(entity ; "wrr: arbitrate {}", self.state_str(inputs));

        let num_inputs = inputs.len();
        let mut selected_candidate = None;
        for i in 0..num_inputs {
            let index = (i + self.candidate) % num_inputs;
            if inputs[index].is_none() {
                continue;
            }
            if self.weights[index] > self.grants[index] {
                selected_candidate = Some(index);
                break;
            } else if selected_candidate.is_none() {
                selected_candidate = Some(index);
            }
        }
        if let Some(index) = selected_candidate {
            if self.weights[index] == self.grants[index] {
                self.grants[index] = 0;
            }
            self.grants[index] += 1;

            let value = inputs[index].take().unwrap();
            self.candidate = (index + 1) % num_inputs;
            Some((index, value))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low = 0,
    Medium,
    High,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Low
    }
}

struct PriorityLevel {
    current_candidate_index: usize,
    candidates: Vec<usize>,
}

pub struct PriorityRoundRobinPolicy<P>
where
    P: Copy + Default + Ord,
{
    priority_vec: Vec<P>,
    priority_map: Option<BTreeMap<P, PriorityLevel>>,
}

impl<P> PriorityRoundRobinPolicy<P>
where
    P: Copy + Default + Ord,
{
    #[must_use]
    pub fn new(num_inputs: usize) -> Self {
        let default_priority = P::default();
        let priority_vec = vec![default_priority; num_inputs];

        Self {
            priority_vec,
            priority_map: None,
        }
    }

    pub fn from_priorities(priority_vec: Vec<P>, num_inputs: usize) -> Result<Self, SimError> {
        if priority_vec.len() != num_inputs {
            return sim_error!("The number of priorities must be equal to the number of inputs");
        }

        Ok(Self {
            priority_vec,
            priority_map: None,
        })
    }

    pub fn set_priority(mut self, port_index: usize, new_priority: P) -> Self {
        self.priority_vec[port_index] = new_priority;
        self
    }

    fn create_map(&mut self) {
        let mut priority_map = BTreeMap::new();
        for (i, priority) in self.priority_vec.iter().enumerate() {
            priority_map
                .entry(*priority)
                .and_modify(|e: &mut PriorityLevel| {
                    e.candidates.push(i);
                })
                .or_insert(PriorityLevel {
                    current_candidate_index: 0,
                    candidates: Vec::from([i]),
                });
        }
        self.priority_map = Some(priority_map);
    }
}

impl<T, P> Arbitrate<T> for PriorityRoundRobinPolicy<P>
where
    T: SimObject,
    P: Copy + Default + Ord,
{
    fn arbitrate(&mut self, _entity: &Arc<Entity>, inputs: &mut [Option<T>]) -> Option<(usize, T)> {
        if self.priority_map.is_none() {
            self.create_map();
        }
        let priority_map = self.priority_map.as_mut().unwrap();
        for (_, priority_level) in priority_map.iter_mut().rev() {
            let priority_vec = &priority_level.candidates;
            let num_inputs = priority_vec.len();
            for i in 0..num_inputs {
                let priority_index = (i + priority_level.current_candidate_index) % num_inputs;
                let input_index = priority_vec[priority_index];
                if let Some(value) = inputs[input_index].take() {
                    priority_level.current_candidate_index = (priority_index + 1) % num_inputs;
                    return Some((input_index, value));
                }
            }
        }
        None
    }
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

        loop {
            let wait_event;
            loop {
                let value;
                let wake_event;
                {
                    // Need to hold the guard for the entire arbitration until the wake_event has
                    // been taken
                    let mut active = self.shared_state.active.borrow_mut();
                    let t = policy.arbitrate(&self.entity, &mut active);
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
                exit!(self.entity ; value.tag());
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
        enter!(entity ; value.tag());

        // Check if this input needs to wait for the previous value to be handled
        let wait_event = match shared_state.active.borrow()[input_idx].as_ref() {
            Some(_) => {
                let once = Once::default();
                *shared_state.waiting_put[input_idx].borrow_mut() = Some(once.clone());
                Some(once)
            }
            None => None,
        };
        if let Some(once) = wait_event {
            once.listen().await;
        }

        // Set the value for this input
        shared_state.active.borrow_mut()[input_idx] = Some(value);

        // Wake up the arbiter if it has paused on an event
        if let Some(once) = shared_state.arbiter_event.borrow_mut().take() {
            once.notify().unwrap();
        }
    }
}
