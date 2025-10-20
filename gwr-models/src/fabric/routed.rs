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
//!  - N [input ports](gwr_engine::port::InPort): `rx[row][column][0, N-1]`
//!  - N [output ports](gwr_engine::port::OutPort): `tx[row][column][0, N-1]`
//!
//! where:
//!  - N = num_ports

use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::connect_port;
use gwr_components::delay::Delay;
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Routable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, Runnable};
use gwr_track::entity::Entity;

use crate::fabric::node::{FabricNode, FabricRoutingAlgoritm};
use crate::fabric::{Fabric, FabricConfig};

#[derive(EntityDisplay, Runnable)]
pub struct RoutedFabric<T>
where
    T: SimObject + Routable,
{
    pub entity: Rc<Entity>,
    nodes: Vec<Vec<Rc<FabricNode<T>>>>,
    config: Rc<FabricConfig>,
}

type FabricNodesResult<T> = Result<Vec<Vec<Rc<FabricNode<T>>>>, SimError>;

fn create_nodes<T>(
    engine: &Engine,
    entity: &Rc<Entity>,
    clock: &Clock,
    config: &Rc<FabricConfig>,
    fabric_algorithm: FabricRoutingAlgoritm,
) -> FabricNodesResult<T>
where
    T: SimObject + Routable,
{
    let num_columns = config.num_columns();
    let num_rows = config.num_rows();
    let mut nodes = Vec::with_capacity(num_columns);

    for col in 0..num_columns {
        let mut col_nodes = Vec::with_capacity(num_rows);
        for row in 0..num_rows {
            let node = FabricNode::new_and_register(
                engine,
                entity,
                format!("node_{col}_{row}").as_str(),
                col,
                row,
                clock.clone(),
                config,
                fabric_algorithm,
            )?;
            col_nodes.push(node);
        }
        nodes.push(col_nodes);
    }
    Ok(nodes)
}

/// Create connections between columns
fn connect_columns<T>(
    engine: &Engine,
    entity: &Rc<Entity>,
    clock: &Clock,
    config: &Rc<FabricConfig>,
    nodes: &[Vec<Rc<FabricNode<T>>>],
    delay_ticks: usize,
) -> Result<(), SimError>
where
    T: SimObject + Routable,
{
    let spawner = engine.spawner();
    for col in 1..config.num_columns {
        let col_m1 = col - 1;
        for row in 0..config.num_rows {
            let delay = Delay::new_and_register(
                engine,
                entity,
                format!("{col_m1}_to_{col}_{row}").as_str(),
                clock.clone(),
                spawner.clone(),
                delay_ticks,
            )?;
            connect_port!(nodes[col_m1][row], col_plus => delay, rx)?;
            connect_port!(delay, tx => nodes[col][row], col_minus)?;

            let delay = Delay::new_and_register(
                engine,
                entity,
                format!("{col}_to_{col_m1}_{row}").as_str(),
                clock.clone(),
                spawner.clone(),
                delay_ticks,
            )?;
            connect_port!(nodes[col][row], col_minus => delay, rx)?;
            connect_port!(delay, tx => nodes[col_m1][row], col_plus)?;
        }
    }
    Ok(())
}

/// Create connections between rows
fn connect_rows<T>(
    engine: &Engine,
    entity: &Rc<Entity>,
    clock: &Clock,
    config: &Rc<FabricConfig>,
    nodes: &[Vec<Rc<FabricNode<T>>>],
    delay_ticks: usize,
) -> Result<(), SimError>
where
    T: SimObject + Routable,
{
    let spawner = engine.spawner();
    for (c, col) in nodes.iter().enumerate() {
        for row in 1..config.num_rows {
            let row_m1 = row - 1;
            let delay = Delay::new_and_register(
                engine,
                entity,
                format!("{c}_{row_m1}_to_{row}").as_str(),
                clock.clone(),
                spawner.clone(),
                delay_ticks,
            )?;
            connect_port!(col[row_m1], row_plus => delay, rx)?;
            connect_port!(delay, tx => col[row], row_minus)?;

            let delay = Delay::new_and_register(
                engine,
                entity,
                format!("{c}_{row}_to_{row_m1}").as_str(),
                clock.clone(),
                spawner.clone(),
                delay_ticks,
            )?;
            connect_port!(col[row], row_minus => delay, rx)?;
            connect_port!(delay, tx => col[row_m1], row_plus)?;
        }
    }
    Ok(())
}

/// Connect up the edge ports that will otherwise be left dangling
fn create_dummy_ports<T>(
    entity: &Rc<Entity>,
    config: &Rc<FabricConfig>,
    nodes: &[Vec<Rc<FabricNode<T>>>],
) -> Result<(), SimError>
where
    T: SimObject + Routable,
{
    // Connect dummy ports left/right
    let right = config.num_columns - 1;
    for row in 0..config.num_rows {
        let mut port = OutPort::new(entity, format!("out_col_dummy_0_{row}").as_str());
        port.connect(nodes[0][row].port_col_minus())?;
        let port = InPort::new(entity, format!("in_col_dummy_0_{row}").as_str());
        nodes[0][row].connect_port_col_minus(port.state())?;
        let mut port = OutPort::new(entity, format!("out_col_dummy_{right}_{row}").as_str());
        port.connect(nodes[right][row].port_col_plus())?;
        let port = InPort::new(entity, format!("in_col_dummy_{right}_{row}").as_str());
        nodes[right][row].connect_port_col_plus(port.state())?;
    }

    // Connect dummy ports top/bottom
    let bottom = config.num_rows - 1;
    for (c, col) in nodes.iter().enumerate() {
        let mut port = OutPort::new(entity, format!("out_row_dummy_{c}_0").as_str());
        port.connect(col[0].port_row_minus())?;
        let port = InPort::new(entity, format!("in_row_dummy_{c}_0").as_str());
        col[0].connect_port_row_minus(port.state())?;
        let mut port = OutPort::new(entity, format!("out_row_dummy_{c}_{bottom}").as_str());
        port.connect(col[bottom].port_row_plus())?;
        let port = InPort::new(entity, format!("in_row_dummy_{c}_{bottom}").as_str());
        col[bottom].connect_port_row_plus(port.state())?;
    }
    Ok(())
}

impl<T> RoutedFabric<T>
where
    T: SimObject + Routable,
{
    pub fn new_and_register(
        engine: &Engine,
        parent: &Rc<Entity>,
        name: &str,
        clock: &Clock,
        config: Rc<FabricConfig>,
        fabric_algorithm: FabricRoutingAlgoritm,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let num_ports = config.num_columns * config.num_rows * config.num_ports_per_node;
        if num_ports < 2 {
            return sim_error!("Cannot create fabric with less than 2 ports");
        }

        let nodes = create_nodes(engine, &entity, clock, &config, fabric_algorithm)?;
        connect_columns(
            engine,
            &entity,
            clock,
            &config,
            &nodes,
            config.cycles_per_hop,
        )?;
        connect_rows(
            engine,
            &entity,
            clock,
            &config,
            &nodes,
            config.cycles_per_hop,
        )?;
        create_dummy_ports(&entity, &config, &nodes)?;

        let rc_self = Rc::new(Self {
            entity,
            nodes,
            config,
        });

        engine.register(rc_self.clone());
        Ok(rc_self)
    }
}

impl<T> Fabric<T> for RoutedFabric<T>
where
    T: SimObject + Routable,
{
    fn connect_port_egress_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult {
        let (col, row, port) = self.config.fabric_port_index_to_col_row_port(i);
        self.nodes[col][row].connect_port_egress_i(port, port_state)
    }

    fn port_ingress_i(&self, i: usize) -> PortStateResult<T> {
        let (col, row, port) = self.config.fabric_port_index_to_col_row_port(i);
        self.nodes[col][row].port_ingress_i(port)
    }
}
