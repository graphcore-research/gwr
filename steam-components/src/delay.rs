// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A component that adds `delay_ticks` between receiving anything and sending
//! it on to its output. The output of the delay should never be blocked. If
//! the output causes the delay to block then this block will `panic!`. If a
//! component is required to support outputs to other components that may block
//! then a flow-controlled component is more suitable as it contains buffering
//! to support this.
//!
//! # Ports
//!
//! This component has two ports:
//!  - One [output port](steam_engine::port::OutPort): `rx`
//!  - One [input port](steam_engine::port::InPort): `tx`

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use steam_engine::events::repeated::Repeated;
use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortState};
use steam_engine::time::clock::{Clock, ClockTick};
use steam_engine::traits::{Event, SimObject};
use steam_engine::types::SimResult;
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::{enter, exit};

use crate::{connect_tx, port_rx, take_option};

struct DelayState<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    clock: Clock,
    delay_ticks: RefCell<usize>,

    rx: RefCell<Option<InPort<T>>>,
    pending: RefCell<VecDeque<(T, ClockTick)>>,
    pending_changed: Repeated<usize>,
    tx: RefCell<Option<OutPort<T>>>,
}

impl<T> DelayState<T>
where
    T: SimObject,
{
    fn new(entity: Arc<Entity>, clock: Clock, delay_ticks: usize) -> Self {
        Self {
            entity: entity.clone(),
            clock,
            delay_ticks: RefCell::new(delay_ticks),
            rx: RefCell::new(Some(InPort::new(entity.clone()))),
            pending: RefCell::new(VecDeque::new()),
            pending_changed: Repeated::new(usize::default()),
            tx: RefCell::new(Some(OutPort::new(entity, "tx"))),
        }
    }
}

#[derive(Clone, EntityDisplay)]
pub struct Delay<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    state: Rc<DelayState<T>>,
}

impl<T> Delay<T>
where
    T: SimObject,
{
    pub fn new(
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        delay_ticks: usize,
    ) -> Self {
        let entity = Arc::new(Entity::new(parent, name));
        Self {
            entity: entity.clone(),
            spawner: spawner.clone(),
            state: Rc::new(DelayState::new(entity, clock, delay_ticks)),
        }
    }

    pub fn connect_port_tx(&self, port_state: Rc<PortState<T>>) {
        connect_tx!(self.state.tx, connect ; port_state);
    }

    pub fn port_rx(&self) -> Rc<PortState<T>> {
        port_rx!(self.state.rx, state)
    }

    pub fn set_delay(&self, delay_ticks: usize) {
        if self.state.rx.borrow().is_none() {
            panic!(
                "{}: can't change the delay after the simulation has started",
                self.entity
            );
        }
        *self.state.delay_ticks.borrow_mut() = delay_ticks;
    }

    pub async fn run(&self) -> SimResult {
        // Spawn the other end of the delay
        let tx = take_option!(self.state.tx);

        let entity = self.entity.clone();
        let state = self.state.clone();
        self.spawner
            .spawn(async move { run_tx(entity, tx, state).await });

        let rx = take_option!(self.state.rx);
        let delay_ticks = *self.state.delay_ticks.borrow() as u64;
        loop {
            let value = rx.get().await;
            let value_tag = value.tag();
            enter!(self.state.entity ; value_tag);

            let mut tick = self.state.clock.tick_now();
            tick.set_tick(tick.tick() + delay_ticks);

            self.state.pending.borrow_mut().push_back((value, tick));
            self.state.pending_changed.notify()?;
        }
    }
}

async fn run_tx<T>(entity: Arc<Entity>, tx: OutPort<T>, state: Rc<DelayState<T>>) -> SimResult
where
    T: SimObject,
{
    loop {
        let next = state.pending.borrow_mut().pop_front();

        match next {
            Some((value, tick)) => {
                let tick_now = state.clock.tick_now();
                match tick.cmp(&tick_now) {
                    Ordering::Greater => {
                        state.clock.wait_ticks(tick.tick() - tick_now.tick()).await;
                    }
                    Ordering::Less => {
                        panic!("Delay output stalled");
                    }
                    Ordering::Equal => {
                        // Do nothing - no need to pause
                    }
                }

                exit!(entity ; value.tag());
                tx.put(value).await?;
            }
            None => {
                state.pending_changed.listen().await;
            }
        }
    }
}
