// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! A functional implementation of a fabric with very basic timing.
//!
//! Assumes that all traffic will move a Manhattan distance through the fabric
//! to get from ingress to egress.
//!
//! The fabric is assumed to be rectangular with a configurable `num_rows` and
//! `num_columns`. The grid has a configurable number of ports at each node
//! within the fabric grid.
//!
//! # Ports
//!
//! Each point in the fabric grid has a configurable numbe
//!  - N [input ports](tramway_engine::port::InPort): `rx[row][column][0, N-1]`
//!  - N [output ports](tramway_engine::port::OutPort): `tx[row][column][0,
//!    N-1]`
//!
//! where:
//!  - N = num_ports

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use tramway_components::flow_controls::limiter::Limiter;
use tramway_components::router::{DefaultAlgorithm, Route};
use tramway_components::store::Store;
use tramway_components::{connect_port, rc_limiter};
use tramway_engine::engine::Engine;
use tramway_engine::events::repeated::Repeated;
use tramway_engine::executor::Spawner;
use tramway_engine::port::{InPort, OutPort, PortStateResult};
use tramway_engine::sim_error;
use tramway_engine::time::clock::{Clock, ClockTick};
use tramway_engine::traits::{Event, Routable, Runnable, SimObject};
use tramway_engine::types::{SimError, SimResult};
use tramway_model_builder::EntityDisplay;
use tramway_track::entity::Entity;
use tramway_track::{enter, exit};

use crate::fabric::FabricConfig;

/// Return the Manhatten time to travel between RX and TX ports specified.
#[must_use]
fn manhatten_rx_to_tx_cycles(
    config: &FabricConfig,
    rx_port_index: usize,
    tx_port_index: usize,
) -> usize {
    let (rx_col, rx_row, _) = config.port_col_row_index(rx_port_index);
    let (tx_col, tx_row, _) = config.port_col_row_index(tx_port_index);
    let horizontal_hops = rx_col.abs_diff(tx_col);
    let vertical_hops = rx_row.abs_diff(tx_row);

    // Add one hop for enterring so that there is never a zero-cycle latency which
    // could otherwise be seen between ports on the same fabric node
    (horizontal_hops + vertical_hops) * config.cycles_per_hop + config.cycles_overhead
}

#[derive(EntityDisplay)]
pub struct Fabric<T>
where
    T: SimObject + Routable,
{
    pub entity: Rc<Entity>,
    rx_buffer_limiters: Vec<Rc<Limiter<T>>>,
    internal_rx: RefCell<Vec<InPort<T>>>,
    tx_buffers: Vec<Rc<Store<T>>>,
    internal_tx: RefCell<Vec<OutPort<T>>>,
    config: Rc<FabricConfig>,
    clock: Clock,
    spawner: Spawner,
}

impl<T> Fabric<T>
where
    T: SimObject + Routable,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Rc<Entity>,
        name: &str,
        clock: Clock,
        spawner: Spawner,
        config: Rc<FabricConfig>,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));

        let num_ports = config.num_columns * config.num_rows * config.num_ports_per_node;
        if num_ports == 0 {
            return sim_error!(format!("Cannot construct fabric with 0 ports"));
        }

        let mut rx_buffer_limiters = Vec::with_capacity(num_ports);
        let mut internal_rx = Vec::with_capacity(num_ports);
        let mut tx_buffers = Vec::with_capacity(num_ports);
        let mut internal_tx = Vec::with_capacity(num_ports);

        let port_limiter = rc_limiter!(clock.clone(), config.port_bits_per_tick);

        for i in 0..num_ports {
            // Build a buffer per input
            let rx_buffer_limiter = Limiter::new_and_register(
                engine,
                &entity,
                format!("limit_rx{i}").as_str(),
                port_limiter.clone(),
            )?;
            let rx_buffer = Store::new_and_register(
                engine,
                &entity,
                format!("rx_buf{i}").as_str(),
                spawner.clone(),
                config.rx_buffer_entries,
            )?;
            connect_port!(rx_buffer_limiter, tx => rx_buffer, rx)?;

            // Create and connect a port to receive from the RX
            let internal_rx_port = InPort::new(&entity, format!("internal_rx{i}").as_str());
            rx_buffer.connect_port_tx(internal_rx_port.state()).unwrap();

            rx_buffer_limiters.push(rx_buffer_limiter);
            internal_rx.push(internal_rx_port);

            // Build a buffer per output
            let tx_buffer_limiter = Limiter::new_and_register(
                engine,
                &entity,
                format!("limit_tx{i}").as_str(),
                port_limiter.clone(),
            )?;
            let tx_buffer = Store::new_and_register(
                engine,
                &entity,
                format!("tx_buf{i}").as_str(),
                spawner.clone(),
                config.tx_buffer_entries,
            )?;
            connect_port!(tx_buffer_limiter, tx => tx_buffer, rx)?;

            // Create and connect a port to drive the TX
            let mut internal_tx_port = OutPort::new(&entity, format!("internal_tx{i}").as_str());
            internal_tx_port.connect(tx_buffer_limiter.port_rx())?;

            tx_buffers.push(tx_buffer);
            internal_tx.push(internal_tx_port);
        }

        let rc_self = Rc::new(Self {
            entity,
            rx_buffer_limiters,
            internal_rx: RefCell::new(internal_rx),
            tx_buffers,
            internal_tx: RefCell::new(internal_tx),
            config,
            clock,
            spawner,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn port_index(&self, col: usize, row: usize, node_index: usize) -> usize {
        self.config.port_index(col, row, node_index)
    }

    pub fn connect_port_tx_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult {
        self.tx_buffers[i].connect_port_tx(port_state)
    }

    pub fn port_rx_i(&self, i: usize) -> PortStateResult<T> {
        self.rx_buffer_limiters[i].port_rx()
    }
}

#[async_trait(?Send)]
impl<T> Runnable for Fabric<T>
where
    T: SimObject + Routable,
{
    async fn run(&self) -> SimResult {
        let num_ports = self.config.num_ports();
        let mut port_states = Vec::with_capacity(num_ports);
        for _ in 0..num_ports {
            port_states.push(PortState::default());
        }
        let port_states = Rc::new(port_states);

        let routing_algorithm: Rc<Box<dyn Route<T>>> = Rc::new(Box::new(DefaultAlgorithm {}));

        for (i, internal_rx) in self.internal_rx.borrow_mut().drain(..).enumerate() {
            let entity = self.entity.clone();
            let clock = self.clock.clone();
            let port_states = port_states.clone();
            let routing_algorithm = routing_algorithm.clone();
            let config = self.config.clone();

            self.spawner.spawn(async move {
                run_rx(
                    entity,
                    clock,
                    i,
                    internal_rx,
                    port_states,
                    routing_algorithm,
                    config,
                )
                .await
            });
        }

        for (i, internal_tx) in self.internal_tx.borrow_mut().drain(..).enumerate() {
            let entity = self.entity.clone();
            let clock = self.clock.clone();
            let port_states = port_states.clone();

            self.spawner
                .spawn(async move { run_tx(entity, clock, i, internal_tx, port_states).await });
        }

        Ok(())
    }
}

/// Structure containing all shared common state for the fabric
///
/// This allows it to be easily shared across all rx and tx handlers.
struct PortState<T> {
    data_for_tx: RefCell<Option<(T, ClockTick)>>,
    waiting_for_data: Repeated<()>,
    waiting_for_room: Repeated<()>,
    inputs_waiting_for_room: RefCell<VecDeque<usize>>,
}

impl<T> Default for PortState<T> {
    fn default() -> Self {
        Self {
            data_for_tx: RefCell::new(None),
            waiting_for_data: Repeated::default(),
            waiting_for_room: Repeated::default(),
            inputs_waiting_for_room: RefCell::new(VecDeque::new()),
        }
    }
}

async fn run_rx<T>(
    entity: Rc<Entity>,
    clock: Clock,
    port_index: usize,
    internal_rx: InPort<T>,
    port_states: Rc<Vec<PortState<T>>>,
    routing_algorithm: Rc<Box<dyn Route<T>>>,
    config: Rc<FabricConfig>,
) -> SimResult
where
    T: SimObject + Routable,
{
    loop {
        let value = internal_rx.get()?.await;
        let value_id = value.id();
        enter!(entity ; value_id);

        let dest_index = routing_algorithm.route(&value)?;
        let delay_ticks = manhatten_rx_to_tx_cycles(&config, port_index, dest_index);

        let mut tick = clock.tick_now();
        tick.set_tick(tick.tick() + delay_ticks as u64);

        // If the destination already has an unhandled object then wait
        while port_states[dest_index].data_for_tx.borrow().is_some() {
            port_states[dest_index]
                .inputs_waiting_for_room
                .borrow_mut()
                .push_back(port_index);
            port_states[port_index].waiting_for_room.listen().await;
        }
        *port_states[dest_index].data_for_tx.borrow_mut() = Some((value, tick));
        port_states[dest_index].waiting_for_data.notify()?;
    }
}

async fn run_tx<T>(
    entity: Rc<Entity>,
    clock: Clock,
    port_index: usize,
    internal_tx: OutPort<T>,
    port_states: Rc<Vec<PortState<T>>>,
) -> SimResult
where
    T: SimObject + Routable,
{
    loop {
        let next = port_states[port_index].data_for_tx.borrow_mut().take();
        match next {
            Some((value, tick)) => {
                let tick_now = clock.tick_now();
                if tick_now.tick() < tick.tick() {
                    // Need to send in the future, delay
                    clock.wait_ticks(tick.tick() - tick_now.tick()).await;
                }
                exit!(entity ; value.id());
                internal_tx.put(value)?.await;

                if let Some(waiting_input) = port_states[port_index]
                    .inputs_waiting_for_room
                    .borrow_mut()
                    .pop_front()
                {
                    port_states[waiting_input].waiting_for_room.notify()?;
                }
            }
            None => {
                port_states[port_index].waiting_for_data.listen().await;
            }
        }
    }
}
