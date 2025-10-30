// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A component that adds `delay_ticks` between receiving anything and sending
//! it on to its output.
//!
//! The `Delay` can be configured such that it will return an error if the
//! output is ever blocked. Otherwise it will implicitly assert back-pressure on
//! the input.
//!
//! # Ports
//!
//! This component has the following ports:
//!  - One [input port](gwr_engine::port::InPort): `rx`
//!  - One [output port](gwr_engine::port::OutPort): `tx`

//! # Function
//!
//! Fundamentally the [Delay]'s functionality is to:
//!
//! ```rust
//! # use std::rc::Rc;
//! # use async_trait::async_trait;
//! # use gwr_engine::port::{InPort, OutPort};
//! # use gwr_engine::sim_error;
//! # use gwr_engine::time::clock::Clock;
//! # use gwr_engine::traits::SimObject;
//! # use gwr_engine::types::SimResult;
//! # use gwr_track::entity::Entity;
//! #
//! # async fn run_tx<T>(
//! #     entity: Rc<Entity>,
//! #     tx: OutPort<T>,
//! #     clock: &Clock,
//! #     rx: InPort<T>,
//! #     delay_ticks: u64,
//! # ) -> SimResult
//! # where
//! #     T: SimObject,
//! # {
//! loop {
//!     let value = rx.get()?.await;
//!     clock.wait_ticks(delay_ticks).await;
//!     tx.put(value)?.await;
//! }
//! # }
//! ```
//!
//! However, the problem with this is that the input ends up being blocked if
//! the output does not instantly consume the value. Therefore the [Delay] is
//! actually split into two halves that manage the ports independently.
//!
//! ## Input
//!
//! A simplified view of how the input side works is:
//!
//! ```rust
//! # use std::cell::RefCell;
//! # use std::collections::VecDeque;
//! # use std::rc::Rc;
//! # use async_trait::async_trait;
//! # use gwr_engine::events::repeated::Repeated;
//! # use gwr_engine::port::{InPort, OutPort};
//! # use gwr_engine::sim_error;
//! # use gwr_engine::time::clock::{Clock, ClockTick};
//! # use gwr_engine::traits::SimObject;
//! # use gwr_engine::types::SimResult;
//! # use gwr_track::entity::Entity;
//! #
//! # async fn run_rx<T>(
//! #     entity: Rc<Entity>,
//! #     rx: InPort<T>,
//! #     clock: &Clock,
//! #     pending: Rc<RefCell<VecDeque<(T, ClockTick)>>>,
//! #     pending_changed: Repeated<usize>,
//! #     delay_ticks: u64,
//! # ) -> SimResult
//! # where
//! #     T: SimObject,
//! # {
//! loop {
//!     // Receive value from input
//!     let value = rx.get()?.await;
//!
//!     // Compute time at which it should leave Delay
//!     let mut tick = clock.tick_now();
//!     tick.set_tick(tick.tick() + delay_ticks as u64);
//!
//!     // Send to the output side
//!     pending.borrow_mut().push_back((value, tick));
//!
//!     // Wake up output if required
//!     pending_changed.notify()?;
//! }
//!  # }
//! ```

//!
//! ## Output
//!
//! A simplified view of how the output side works is:
//!
//! ```rust
//! # use std::cell::RefCell;
//! # use std::collections::VecDeque;
//! # use std::rc::Rc;
//! # use async_trait::async_trait;
//! # use gwr_engine::events::repeated::Repeated;
//! # use gwr_engine::port::{InPort, OutPort};
//! # use gwr_engine::sim_error;
//! # use gwr_engine::time::clock::{Clock, ClockTick};
//! # use gwr_engine::traits::{Event, SimObject};
//! # use gwr_engine::types::SimResult;
//! # use gwr_track::entity::Entity;
//! #
//! # async fn run_tx<T>(
//! #     entity: Rc<Entity>,
//! #     tx: OutPort<T>,
//! #     clock: &Clock,
//! #     pending: Rc<RefCell<VecDeque<(T, ClockTick)>>>,
//! #     pending_changed: Repeated<usize>,
//! # ) -> SimResult
//! # where
//! #     T: SimObject,
//! # {
//! loop {
//!     // Get next value and tick at which to send value
//!     if let Some((value, tick)) = pending.borrow_mut().pop_front() {
//!         // Wait for correct time
//!         let tick_now = clock.tick_now();
//!         clock.wait_ticks(tick.tick() - tick_now.tick()).await;
//!
//!         // Send value
//!         tx.put(value)?.await;
//!     } else {
//!         // Wait to be notified of new data
//!         pending_changed.listen().await;
//!     }
//! }
//! # }
//! ```
//!
//! ## Using a [Delay]
//!
//! A [Delay] simply needs to be created with the latency through it and
//! connected between components.
//!
//! ```rust
//! # use std::cell::RefCell;
//! # use std::rc::Rc;
//! #
//! # use gwr_components::delay::Delay;
//! # use gwr_components::sink::Sink;
//! # use gwr_components::source::Source;
//! # use gwr_components::store::Store;
//! # use gwr_components::{connect_port, option_box_repeat};
//! # use gwr_engine::engine::Engine;
//! # use gwr_engine::port::{InPort, OutPort};
//! # use gwr_engine::run_simulation;
//! # use gwr_engine::test_helpers::start_test;
//! # use gwr_engine::time::clock::Clock;
//! # use gwr_engine::traits::SimObject;
//! # use gwr_engine::types::SimResult;
//! #
//! # fn source_sink() -> SimResult {
//! #     let mut engine = start_test(file!());
//! #     let clock = engine.default_clock();
//! #
//! #     let delay_ticks = 3;
//! #     let num_puts = delay_ticks * 10;
//! #
//! #     let top = engine.top();
//! #     let to_send: Option<Box<dyn Iterator<Item = _>>> = option_box_repeat!(500 ; num_puts);
//!     // Create the components
//!     let source = Source::new_and_register(&engine, top, "source", to_send)?;
//!     let delay = Delay::new_and_register(&engine, &clock, top, "delay", delay_ticks)?;
//!     let sink = Sink::new_and_register(&engine, &clock, top, "sink")?;
//!
//!     // Connect the ports
//!     connect_port!(source, tx => delay, rx)?;
//!     connect_port!(delay, tx => sink, rx)?;
//!
//!     run_simulation!(engine);
//! #
//! #     let num_sunk = sink.num_sunk();
//! #     assert_eq!(num_sunk, num_puts);
//! #     Ok(())
//! # }
//! ```
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::executor::Spawner;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::{Clock, ClockTick};
use gwr_engine::traits::{Event, Runnable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::EntityDisplay;
use gwr_track::entity::Entity;
use gwr_track::tracker::aka::Aka;
use gwr_track::{enter, exit};

use crate::{connect_tx, port_rx, take_option};

#[derive(EntityDisplay)]
pub struct Delay<T>
where
    T: SimObject,
{
    pub entity: Rc<Entity>,
    spawner: Spawner,
    clock: Clock,
    delay_ticks: RefCell<usize>,

    rx: RefCell<Option<InPort<T>>>,
    pending: Rc<RefCell<VecDeque<(T, ClockTick)>>>,
    pending_changed: Repeated<()>,
    output_changed: Repeated<()>,
    tx: RefCell<Option<OutPort<T>>>,

    error_on_output_stall: RefCell<bool>,
}

impl<T> Delay<T>
where
    T: SimObject,
{
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        delay_ticks: usize,
    ) -> Result<Rc<Self>, SimError> {
        let spawner = engine.spawner();
        let entity = Rc::new(Entity::new(parent, name));
        let tx = OutPort::new_with_renames(&entity, "tx", aka);
        let rx = InPort::new_with_renames(engine, clock, &entity, "rx", aka);
        let rc_self = Rc::new(Self {
            entity,
            spawner,
            clock: clock.clone(),
            delay_ticks: RefCell::new(delay_ticks),
            rx: RefCell::new(Some(rx)),
            pending: Rc::new(RefCell::new(VecDeque::new())),
            pending_changed: Repeated::default(),
            output_changed: Repeated::default(),
            tx: RefCell::new(Some(tx)),
            error_on_output_stall: RefCell::new(false),
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        delay_ticks: usize,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(engine, clock, parent, name, None, delay_ticks)
    }

    pub fn set_error_on_output_stall(&self) {
        *self.error_on_output_stall.borrow_mut() = true;
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
        let output_changed = self.output_changed.clone();
        let error_on_output_stall = *self.error_on_output_stall.borrow();
        self.spawner.spawn(async move {
            run_tx(
                entity,
                tx,
                &clock,
                pending,
                pending_changed,
                output_changed,
                error_on_output_stall,
            )
            .await
        });

        let rx = take_option!(self.rx);
        let delay_ticks = *self.delay_ticks.borrow();
        loop {
            let value = rx.get()?.await;
            let value_id = value.id();
            enter!(self.entity ; value_id);

            let mut tick = self.clock.tick_now();
            tick.set_tick(tick.tick() + delay_ticks as u64);

            self.pending.borrow_mut().push_back((value, tick));
            self.pending_changed.notify()?;

            if delay_ticks > 0 && !*self.error_on_output_stall.borrow() {
                // Enforce back-pressure by waiting until there is room in the pending queue
                while self.pending.borrow().len() >= delay_ticks {
                    self.output_changed.listen().await;
                }
            }
        }
    }
}

async fn run_tx<T>(
    entity: Rc<Entity>,
    tx: OutPort<T>,
    clock: &Clock,
    pending: Rc<RefCell<VecDeque<(T, ClockTick)>>>,
    pending_changed: Repeated<()>,
    output_changed: Repeated<()>,
    error_on_output_stall: bool,
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
                        if error_on_output_stall {
                            return sim_error!(format!("{entity} delay output stalled"));
                        }
                    }
                    Ordering::Equal => {
                        // Do nothing - no need to pause
                    }
                }

                exit!(entity ; value.id());
                tx.put(value)?.await;
                output_changed.notify()?;
            }
            None => {
                pending_changed.listen().await;
            }
        }
    }
}
