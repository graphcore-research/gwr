// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A component that adds `delay_ticks` between receiving anything and sending
//! it on to its output. The output of the delay should never be blocked. If
//! the output causes the delay to block then this block will return a
//! `SimError`. If a component is required to support outputs to other
//! components that may block then a flow-controlled component is more suitable
//! as it contains buffering to support this.
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

use async_trait::async_trait;
use steam_engine::engine::Engine;
use steam_engine::events::repeated::Repeated;
use steam_engine::executor::Spawner;
use steam_engine::port::{InPort, OutPort, PortStateResult};
use steam_engine::sim_error;
use steam_engine::time::clock::{Clock, ClockTick};
use steam_engine::traits::{Event, Runnable, SimObject};
use steam_engine::types::{SimError, SimResult};
use steam_model_builder::EntityDisplay;
use steam_track::entity::Entity;
use steam_track::{enter, exit};

use crate::{connect_tx, port_rx, take_option};

#[derive(EntityDisplay)]
pub struct Delay<T>
where
    T: SimObject,
{
    pub entity: Arc<Entity>,
    spawner: Spawner,
    clock: Clock,
    delay_ticks: RefCell<usize>,

    rx: RefCell<Option<InPort<T>>>,
    pending: Rc<RefCell<VecDeque<(T, ClockTick)>>>,
    pending_changed: Repeated<usize>,
    tx: RefCell<Option<OutPort<T>>>,
}

impl<T> Delay<T>
where
    T: SimObject,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Arc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        delay_ticks: usize,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Arc::new(Entity::new(parent, name));
        let tx = OutPort::new(&entity, "tx");
        let rx = InPort::new(&entity, "rx");
        let rc_self = Rc::new(Self {
            entity,
            spawner,
            clock,
            delay_ticks: RefCell::new(delay_ticks),
            rx: RefCell::new(Some(rx)),
            pending: Rc::new(RefCell::new(VecDeque::new())),
            pending_changed: Repeated::new(usize::default()),
            tx: RefCell::new(Some(tx)),
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

    pub fn set_delay(&self, delay_ticks: usize) -> SimResult {
        if self.rx.borrow().is_none() {
            return sim_error!(format!(
                "{}: can't change the delay after the simulation has started",
                self.entity
            ));
        }
        *self.delay_ticks.borrow_mut() = delay_ticks;
        Ok(())
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Delay<T>
where
    T: SimObject,
{
    async fn run(&self) -> SimResult {
        // Spawn the other end of the delay
        let tx = take_option!(self.tx);

        let entity = self.entity.clone();
        let clock = self.clock.clone();
        let pending = self.pending.clone();
        let pending_changed = self.pending_changed.clone();
        self.spawner
            .spawn(async move { run_tx(entity, tx, clock, pending, pending_changed).await });

        let rx = take_option!(self.rx);
        let delay_ticks = *self.delay_ticks.borrow() as u64;
        loop {
            let value = rx.get()?.await;
            let value_tag = value.tag();
            enter!(self.entity ; value_tag);

            let mut tick = self.clock.tick_now();
            tick.set_tick(tick.tick() + delay_ticks);

            self.pending.borrow_mut().push_back((value, tick));
            self.pending_changed.notify()?;
        }
    }
}

async fn run_tx<T>(
    entity: Arc<Entity>,
    tx: OutPort<T>,
    clock: Clock,
    pending: Rc<RefCell<VecDeque<(T, ClockTick)>>>,
    pending_changed: Repeated<usize>,
) -> SimResult
where
    T: SimObject,
{
    loop {
        let next = pending.borrow_mut().pop_front();

        match next {
            Some((value, tick)) => {
                let tick_now = clock.tick_now();
                match tick.cmp(&tick_now) {
                    Ordering::Greater => {
                        clock.wait_ticks(tick.tick() - tick_now.tick()).await;
                    }
                    Ordering::Less => {
                        return sim_error!("Delay output stalled");
                    }
                    Ordering::Equal => {
                        // Do nothing - no need to pause
                    }
                }

                exit!(entity ; value.tag());
                tx.put(value)?.await;
            }
            None => {
                pending_changed.listen().await;
            }
        }
    }
}
