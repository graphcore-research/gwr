// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! A functional implementation of a fabric with very basic timing.
//!
//! Assumes that all traffic will move a Manhattan distance through the fabric
//! to get from ingress to egress.
//!
//! The fabric is assumed to be rectangular with a configurable `num_columns`
//! and `num_rows`. The grid has a configurable number of ports at each node
//! within the fabric grid.
//!
//! # Ports
//!
//! Each point in the fabric grid (col, row) has N ingress and egress ports:
//!  - N [ingress ports](gwr_engine::port::InPort): `ingress[col][row][0, N-1]`
//!  - N [egress ports](gwr_engine::port::OutPort): `egress[col][row][0, N-1]`
//!
//! In order to connect to the fabric use the
//! `col_row_port_to_fabric_port_index()` function in the configuration
//! structure to get the index of the port you want to connect to.

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
use gwr_track::tracker::aka::{Aka, populate_aka_from_string};

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

fn build_node_aka(
    entity: &Rc<Entity>,
    aka: Option<&Aka>,
    new_aka: &mut Aka,
    col: usize,
    row: usize,
    config: &Rc<FabricConfig>,
) {
    let mut renames = Vec::new();
    for port in 0..config.num_ports_per_node() {
        let fabric_port_index = config.col_row_port_to_fabric_port_index(col, row, port);
        renames.push((
            format!("ingress_{fabric_port_index}"),
            format!("ingress_{port}"),
        ));
        renames.push((
            format!("egress_{fabric_port_index}"),
            format!("egress_{port}"),
        ));
    }
    populate_aka_from_string(aka, Some(new_aka), entity, &renames);
}

type FabricNodesResult<T> = Result<Vec<Vec<Rc<FabricNode<T>>>>, SimError>;

fn create_nodes<T>(
    engine: &Engine,
    clock: &Clock,
    entity: &Rc<Entity>,
    aka: Option<&Aka>,
    config: &Rc<FabricConfig>,
    fabric_algorithm: FabricRoutingAlgoritm,
) -> FabricNodesResult<T>
where
    T: SimObject + Routable,
{
    let num_columns = config.num_columns();
    let num_rows = config.num_rows();
    let mut nodes = Vec::with_capacity(num_columns);

    for c in 0..num_columns {
        let mut col_nodes = Vec::with_capacity(num_rows);
        for r in 0..num_rows {
            let mut new_aka = Aka::default();
            build_node_aka(entity, aka, &mut new_aka, c, r, config);
            let node = FabricNode::new_and_register_with_renames(
                engine,
                clock,
                entity,
                &format!("node_{c}_{r}"),
                Some(&new_aka),
                c,
                r,
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
    clock: &Clock,
    entity: &Rc<Entity>,
    config: &Rc<FabricConfig>,
    nodes: &[Vec<Rc<FabricNode<T>>>],
    delay_ticks: usize,
) -> Result<(), SimError>
where
    T: SimObject + Routable,
{
    for c in 1..config.num_columns {
        let c_m1 = c - 1;
        for r in 0..config.num_rows {
            let delay = Delay::new_and_register(
                engine,
                clock,
                entity,
                &format!("{c_m1}_{r}_to_{c}_{r}"),
                delay_ticks,
            )?;
            connect_port!(nodes[c_m1][r], col_plus => delay, rx)?;
            connect_port!(delay, tx => nodes[c][r], col_minus)?;

            let delay = Delay::new_and_register(
                engine,
                clock,
                entity,
                &format!("{c}_{r}_to_{c_m1}_{r}"),
                delay_ticks,
            )?;
            connect_port!(nodes[c][r], col_minus => delay, rx)?;
            connect_port!(delay, tx => nodes[c_m1][r], col_plus)?;
        }
    }
    Ok(())
}

/// Create connections between rows
fn connect_rows<T>(
    engine: &Engine,
    clock: &Clock,
    entity: &Rc<Entity>,
    config: &Rc<FabricConfig>,
    nodes: &[Vec<Rc<FabricNode<T>>>],
    delay_ticks: usize,
) -> Result<(), SimError>
where
    T: SimObject + Routable,
{
    for (c, col) in nodes.iter().enumerate() {
        for r in 1..config.num_rows {
            let r_m1 = r - 1;
            let delay = Delay::new_and_register(
                engine,
                clock,
                entity,
                &format!("{c}_{r_m1}_to_{c}_{r}"),
                delay_ticks,
            )?;
            connect_port!(col[r_m1], row_plus => delay, rx)?;
            connect_port!(delay, tx => col[r], row_minus)?;

            let delay = Delay::new_and_register(
                engine,
                clock,
                entity,
                &format!("{c}_{r}_to_{c}_{r_m1}"),
                delay_ticks,
            )?;
            connect_port!(col[r], row_minus => delay, rx)?;
            connect_port!(delay, tx => col[r_m1], row_plus)?;
        }
    }
    Ok(())
}

/// Connect up the edge ports that will otherwise be left dangling
fn create_dummy_ports<T>(
    engine: &Engine,
    clock: &Clock,
    entity: &Rc<Entity>,
    config: &Rc<FabricConfig>,
    nodes: &[Vec<Rc<FabricNode<T>>>],
) -> Result<(), SimError>
where
    T: SimObject + Routable,
{
    // Connect dummy ports left/right
    let right = config.num_columns - 1;
    for r in 0..config.num_rows {
        let mut port = OutPort::new(entity, &format!("out_col_dummy_0_{r}"));
        port.connect(nodes[0][r].port_col_minus())?;
        let port = InPort::new(engine, clock, entity, &format!("in_col_dummy_0_{r}"));
        nodes[0][r].connect_port_col_minus(port.state())?;
        let mut port = OutPort::new(entity, &format!("out_col_dummy_{right}_{r}"));
        port.connect(nodes[right][r].port_col_plus())?;
        let port = InPort::new(engine, clock, entity, &format!("in_col_dummy_{right}_{r}"));
        nodes[right][r].connect_port_col_plus(port.state())?;
    }

    // Connect dummy ports top/bottom
    let bottom = config.num_rows - 1;
    for (c, col) in nodes.iter().enumerate() {
        let mut port = OutPort::new(entity, &format!("out_row_dummy_{c}_0"));
        port.connect(col[0].port_row_minus())?;
        let port = InPort::new(engine, clock, entity, &format!("in_row_dummy_{c}_0"));
        col[0].connect_port_row_minus(port.state())?;
        let mut port = OutPort::new(entity, &format!("out_row_dummy_{c}_{bottom}"));
        port.connect(col[bottom].port_row_plus())?;
        let port = InPort::new(engine, clock, entity, &format!("in_row_dummy_{c}_{bottom}"));
        col[bottom].connect_port_row_plus(port.state())?;
    }
    Ok(())
}

impl<T> RoutedFabric<T>
where
    T: SimObject + Routable,
{
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        config: Rc<FabricConfig>,
        fabric_algorithm: FabricRoutingAlgoritm,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));
        let num_ports = config.num_columns * config.num_rows * config.num_ports_per_node;
        if num_ports < 2 {
            return sim_error!("Cannot create fabric with less than 2 ports");
        }

        let nodes = create_nodes(engine, clock, &entity, aka, &config, fabric_algorithm)?;
        connect_columns(
            engine,
            clock,
            &entity,
            &config,
            &nodes,
            config.cycles_per_hop,
        )?;
        connect_rows(
            engine,
            clock,
            &entity,
            &config,
            &nodes,
            config.cycles_per_hop,
        )?;
        create_dummy_ports(engine, clock, &entity, &config, &nodes)?;

        let rc_self = Rc::new(Self {
            entity,
            nodes,
            config,
        });

        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: Rc<FabricConfig>,
        fabric_algorithm: FabricRoutingAlgoritm,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(
            engine,
            clock,
            parent,
            name,
            None,
            config,
            fabric_algorithm,
        )
    }
}

impl<T> Fabric<T> for RoutedFabric<T>
where
    T: SimObject + Routable,
{
    fn connect_port_egress_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult {
        let (c, r, p) = self.config.fabric_port_index_to_col_row_port(i);
        self.nodes[c][r].connect_port_egress_i(p, port_state)
    }

    fn port_ingress_i(&self, i: usize) -> PortStateResult<T> {
        let (c, r, p) = self.config.fabric_port_index_to_col_row_port(i);
        self.nodes[c][r].port_ingress_i(p)
    }
}
