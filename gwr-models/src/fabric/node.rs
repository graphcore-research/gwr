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
//! Each point in the fabric grid has a configurable number
//!  - N [input ports](gwr_engine::port::InPort): `rx[row][column][0, N-1]`
//!  - N [output ports](gwr_engine::port::OutPort): `tx[row][column][0, N-1]`
//!
//! where:
//!  - N = number of ingress/egress ports.
//!
//! The Node is constructed with fabric row and column ports in
//! addition to the N ingress/egress ports:
//! ```txt
//! +-------------------------------------------------------+
//! | ingress[0..N-1]       row_minus                       |
//! |                                                       |
//! | col_minus                                    col_plus |
//! |                                                       |
//! | egress[0..N-1]        row_plus                        |
//! +-------------------------------------------------------+
//! ```

//! A full fabric model is built of existing limiters, buffers, arbiters and
//! routers such that the path of a frame from ingress to egress could look
//! like:
//!
//! ```txt
//!          +-------------------------------------+       +-------------------------------------+
//!          |              NODE0                  |       |                 NODE1               |
//!  INGRESS -> LIMIT -> BUF -> ROUTER -> ARBITER -> DELAY -> ROUTER -> ARBITER -> LIMIT -> BUF -> EGRESS
//!          |                                     |       |                                     |
//!          +-------------------------------------+       +-------------------------------------+
//! ```

//! Each [Router] performs the task of taking the frame from an input and
//! deciding which arbiter to send the frame to. For example, if there were
//! two fabric ingress/egress ports per node then the router at one of those
//! ingress ports would look like:
//!
//! ```txt
//!             +--------------------------------------------+
//!             |                  NODE                      |
//!             |               +---------------+            |
//!             |               |     ROUTER    |  ARBITERS  |
//!             |               |               |            |
//!             |               |     /-> tx[0] -> col_minus |
//!             |               |     +-> tx[1] -> col_plus  |
//!  ingress[0] -> LIMIT -> BUF -> rx +-> tx[2] -> row_minus |
//!             |               |     +-> tx[3] -> row_plus  |
//!             |               |     \-> tx[4] -> egress[1] |
//!             |               +---------------+            |
//!             +--------------------------------------------+
//! ```

//! Each [Arbiter] does the job of deciding which frame to send next from
//! those available on their inputs. For example, again considering a fabric
//! with two ingress/egress ports per node, the arbiter at one of the egress
//! ports would look like:
//!
//! ```txt
//!  +-------------------------------------------+
//!  |                    NODE                   |
//!  |           +---------------+               |
//!  |  ROUTERS  |     ARBITER   |               |
//!  |           |               |               |
//!  | col_minus -> rx[0] \      |               |
//!  | col_plus  -> rx[1] +      |               |
//!  | row_minus -> rx[2] +-> tx -> LIMIT -> BUF -> egress[0]
//!  | row_plus  -> rx[3] +      |               |
//!  | egress[1] -> rx[4] /      |               |
//!  |           +---------------+               |
//!  +-------------------------------------------+
//! ```

use std::fmt;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::RoundRobin;
use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::router::{Route, Router};
use gwr_components::store::Store;
use gwr_components::{connect_port, rc_limiter};
use gwr_engine::engine::Engine;
use gwr_engine::port::PortStateResult;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Routable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet, Runnable};
use gwr_track::build_aka;
use gwr_track::entity::{Entity, GetEntity};
use gwr_track::tracker::aka::Aka;
use serde::{Deserialize, Serialize};

use crate::fabric::FabricConfig;

#[derive(clap::ValueEnum, Clone, Copy, Default, Debug, Serialize, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FabricRoutingAlgorithm {
    #[default]
    /// Route packets to the right column first
    ColumnFirst,

    /// Route packets to the right row first
    RowFirst,
}

struct NodeRouter {
    index: usize,
    node_col: usize,
    node_row: usize,
    fabric_algorithm: FabricRoutingAlgorithm,
    config: Rc<FabricConfig>,
}

impl<T> Route<T> for NodeRouter
where
    T: SimObject + Routable,
{
    /// Route an object to the right egress port on the router. The [FabricNode]
    /// is constructed with [Arbiter]s and [Router]s that have N-1 ports
    /// (where N is the total number of ports on the [FabricNode]). There
    /// are N-1 ports because it is invalid to route to oneself.
    ///
    /// As a result it is necessary to remap indices from the computed egress
    /// port to the router port. This depends on the index of this router.
    fn route(&self, object: &T) -> Result<usize, SimError> {
        let dest_fabric_port = object.destination() as usize;
        let (dest_col, dest_row, dest_port) = self
            .config
            .fabric_port_index_to_col_row_port(dest_fabric_port);
        let dest_port = if (self.node_col == dest_col) && (self.node_row == dest_row) {
            // Local egress
            dest_port + (Port::Ingress as usize)
        } else if self.node_col == dest_col {
            // Column reached, route by row.
            if self.node_row < dest_row {
                Port::RowPlus as usize
            } else {
                Port::RowMinus as usize
            }
        } else if self.node_row == dest_row {
            // Row reached, route by column.
            if self.node_col < dest_col {
                Port::ColPlus as usize
            } else {
                Port::ColMinus as usize
            }
        } else {
            // Both row/column not reached. Route according to algorithm.
            match self.fabric_algorithm {
                FabricRoutingAlgorithm::ColumnFirst => {
                    if self.node_col < dest_col {
                        Port::ColPlus as usize
                    } else {
                        Port::ColMinus as usize
                    }
                }
                FabricRoutingAlgorithm::RowFirst => {
                    if self.node_row < dest_row {
                        Port::RowPlus as usize
                    } else {
                        Port::RowMinus as usize
                    }
                }
            }
        };

        assert_ne!(
            dest_port, self.index,
            "cannot route frame to egress from same port as ingress"
        );

        // Given there are N-1 ports in routers because they can't route
        // to themselves we need to exclude the self index.
        // For example, if there are two ingress/egress ports then different
        // remappings would look like:
        //           | port  | Remapped indices with self.index
        // name      | index | 0, 1, 2, 3, 4, 5
        // ----------|-------|---------------------------------
        // col_minus | 0     | -, 0, 0, 0, 0, 0,
        // col_plus  | 1     | 0, -, 1, 1, 1, 1,
        // row_minus | 2     | 1, 1, -, 2, 2, 2,
        // row_plus  | 3     | 2, 2, 2, -, 3, 3,
        // egress[0] | 4     | 3, 3, 3, 3, -, 4,
        // egress[1] | 5     | 4, 4, 4, 4, 4, -,
        if dest_port > self.index {
            Ok(dest_port - 1)
        } else {
            Ok(dest_port)
        }
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug)]
pub enum Port {
    ColMinus = 0,
    ColPlus,
    RowMinus,
    RowPlus,
    Ingress,
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // For to_string() use a name in the form of other entities
        let name = match self {
            Port::ColMinus => "col_minus",
            Port::ColPlus => "col_plus",
            Port::RowMinus => "row_minus",
            Port::RowPlus => "row_plus",
            Port::Ingress => "???",
        };
        write!(f, "{name}")
    }
}

type RouterArbiterResult<T> = Result<(Rc<Arbiter<T>>, Rc<Router<T>>), SimError>;

#[expect(clippy::too_many_arguments)]
fn router_arbiter<T>(
    engine: &Engine,
    clock: &Clock,
    node: &Rc<Entity>,
    config: Rc<FabricConfig>,
    fabric_algorithm: FabricRoutingAlgorithm,
    num_arbiter_router_ports: usize,
    router_arbiter_index: usize,
    node_col: usize,
    node_row: usize,
    name: &str,
) -> RouterArbiterResult<T>
where
    T: SimObject + Routable,
{
    let policy = Box::new(RoundRobin::new());
    let algorithm = Box::new(NodeRouter {
        index: router_arbiter_index,
        node_col,
        node_row,
        fabric_algorithm,
        config,
    });
    Ok((
        Arbiter::new_and_register(
            engine,
            clock,
            node,
            &format!("arb_{name}"),
            num_arbiter_router_ports,
            policy,
        )?,
        Router::new_and_register(
            engine,
            clock,
            node,
            &format!("router_{name}"),
            num_arbiter_router_ports,
            algorithm,
        )?,
    ))
}

type Arbiters<T> = Vec<Rc<Arbiter<T>>>;
type Routers<T> = Vec<Rc<Router<T>>>;
type RoutersArbitersResult<T> = Result<(Arbiters<T>, Routers<T>), SimError>;

#[expect(clippy::too_many_arguments)]
fn create_arbiters_routers<T>(
    engine: &Engine,
    clock: &Clock,
    node: &Rc<Entity>,
    config: &Rc<FabricConfig>,
    fabric_algorithm: FabricRoutingAlgorithm,
    num_ingress_egress_ports: usize,
    node_col: usize,
    node_row: usize,
) -> RoutersArbitersResult<T>
where
    T: SimObject + Routable,
{
    let num_arbiters_routers = Port::Ingress as usize + num_ingress_egress_ports;

    // No need to route to self
    let num_arbiter_router_ports = num_arbiters_routers - 1;

    let mut arbiters = Vec::with_capacity(num_arbiters_routers);
    let mut routers = Vec::with_capacity(num_arbiters_routers);

    for (i, port) in vec![Port::ColMinus, Port::ColPlus, Port::RowMinus, Port::RowPlus]
        .drain(..)
        .enumerate()
    {
        let name = port.to_string();
        let (arbiter, router) = router_arbiter(
            engine,
            clock,
            node,
            config.clone(),
            fabric_algorithm,
            num_arbiter_router_ports,
            i,
            node_col,
            node_row,
            name.as_str(),
        )?;
        arbiters.push(arbiter);
        routers.push(router);
    }

    for i in 0..num_ingress_egress_ports {
        let ingress_egress_index = i + Port::Ingress as usize;
        let policy = Box::new(RoundRobin::new());
        arbiters.push(Arbiter::new_and_register(
            engine,
            clock,
            node,
            &format!("arb_{ingress_egress_index}"),
            num_arbiter_router_ports,
            policy,
        )?);
        let algorithm = Box::new(NodeRouter {
            index: ingress_egress_index,
            node_col,
            node_row,
            fabric_algorithm,
            config: config.clone(),
        });
        routers.push(Router::new_and_register(
            engine,
            clock,
            node,
            &format!("router_{ingress_egress_index}"),
            num_arbiter_router_ports,
            algorithm,
        )?);
    }

    Ok((arbiters, routers))
}

type IngressEgressBuffersResult<T> = Result<(Vec<Rc<Limiter<T>>>, Vec<Rc<Store<T>>>), SimError>;

#[expect(clippy::too_many_arguments)]
fn create_ingress_egress_buffers<T>(
    engine: &Engine,
    clock: &Clock,
    node: &Rc<Entity>,
    aka: Option<&Aka>,
    config: &Rc<FabricConfig>,
    num_ingress_egress_ports: usize,
    arbiters: &Arbiters<T>,
    routers: &Routers<T>,
) -> IngressEgressBuffersResult<T>
where
    T: SimObject + Routable,
{
    let mut ingress_buffer_limiters = Vec::with_capacity(num_ingress_egress_ports);
    let mut egress_buffers = Vec::with_capacity(num_ingress_egress_ports);

    let port_limiter = rc_limiter!(clock, config.port_bits_per_tick);
    for i in 0..num_ingress_egress_ports {
        let ingress_egress_index = Port::Ingress as usize + i;
        // Build a buffer per input
        let ingress_buffer_limiter_aka = build_aka!(aka, node, &[(&format!("ingress_{i}"), "rx")]);
        let ingress_buffer_limiter = Limiter::new_and_register_with_renames(
            engine,
            clock,
            node,
            &format!("limit_ingress_{i}"),
            Some(&ingress_buffer_limiter_aka),
            port_limiter.clone(),
        )?;
        let ingress_buffer = Store::new_and_register(
            engine,
            clock,
            node,
            &format!("ingress_buf_{i}"),
            config.rx_buffer_entries,
        )?;
        connect_port!(ingress_buffer_limiter, tx => ingress_buffer, rx)?;
        connect_port!(ingress_buffer, tx => routers[ingress_egress_index], rx)?;
        ingress_buffer_limiters.push(ingress_buffer_limiter);

        // Build a buffer per output
        let egress_buffer_limiter = Limiter::new_and_register(
            engine,
            clock,
            node,
            &format!("limit_egress_{i}"),
            port_limiter.clone(),
        )?;
        let egress_buffer_aka = build_aka!(aka, node, &[(&format!("egress_{i}"), "tx")]);
        let egress_buffer = Store::new_and_register_with_renames(
            engine,
            clock,
            node,
            &format!("egress_buf_{i}"),
            Some(&egress_buffer_aka),
            config.tx_buffer_entries,
        )?;
        connect_port!(egress_buffer_limiter, tx => egress_buffer, rx)?;
        connect_port!(arbiters[ingress_egress_index], tx => egress_buffer_limiter, rx)?;
        egress_buffers.push(egress_buffer);
    }

    Ok((ingress_buffer_limiters, egress_buffers))
}

#[derive(EntityGet, EntityDisplay, Runnable)]
pub struct FabricNode<T>
where
    T: SimObject + Routable,
{
    entity: Rc<Entity>,

    arbiters: Vec<Rc<Arbiter<T>>>,
    routers: Vec<Rc<Router<T>>>,

    ingress_buffer_limiters: Vec<Rc<Limiter<T>>>,
    egress_buffers: Vec<Rc<Store<T>>>,
}

impl<T> FabricNode<T>
where
    T: SimObject + Routable,
{
    #[expect(clippy::too_many_arguments)]
    pub fn new_and_register_with_renames(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        node_col: usize,
        node_row: usize,
        config: &Rc<FabricConfig>,
        fabric_algorithm: FabricRoutingAlgorithm,
    ) -> Result<Rc<Self>, SimError> {
        let entity = Rc::new(Entity::new(parent, name));

        let num_ingress_egress_ports = config.node_num_ingress_egress_ports(node_col, node_row);

        let (arbiters, routers) = create_arbiters_routers(
            engine,
            clock,
            &entity,
            config,
            fabric_algorithm,
            num_ingress_egress_ports,
            node_col,
            node_row,
        )?;

        let (ingress_buffer_limiters, egress_buffers) = create_ingress_egress_buffers(
            engine,
            clock,
            &entity,
            aka,
            config,
            num_ingress_egress_ports,
            &arbiters,
            &routers,
        )?;

        // Perform internal connections from routers -> arbiters
        for (from, router) in routers.iter().enumerate() {
            for (to, arbiter) in arbiters.iter().enumerate() {
                if from == to {
                    continue;
                }

                let to_index = if to > from { to - 1 } else { to };
                let from_index = if from > to { from - 1 } else { from };
                connect_port!(router, tx, to_index => arbiter, rx, from_index)?;
            }
        }

        let rc_self = Rc::new(Self {
            entity,
            ingress_buffer_limiters,
            egress_buffers,
            arbiters,
            routers,
        });
        engine.register(rc_self.clone());
        Ok(rc_self)
    }

    #[expect(clippy::too_many_arguments)]
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        node_col: usize,
        node_row: usize,
        config: &Rc<FabricConfig>,
        fabric_algorithm: FabricRoutingAlgorithm,
    ) -> Result<Rc<Self>, SimError> {
        Self::new_and_register_with_renames(
            engine,
            clock,
            parent,
            name,
            None,
            node_col,
            node_row,
            config,
            fabric_algorithm,
        )
    }

    pub fn connect_port_egress_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult {
        self.egress_buffers[i].connect_port_tx(port_state)
    }
    pub fn port_ingress_i(&self, i: usize) -> PortStateResult<T> {
        self.ingress_buffer_limiters[i].port_rx()
    }

    pub fn connect_port_row_minus(&self, port_state: PortStateResult<T>) -> SimResult {
        self.arbiters[Port::RowMinus as usize].connect_port_tx(port_state)
    }
    pub fn connect_port_row_plus(&self, port_state: PortStateResult<T>) -> SimResult {
        self.arbiters[Port::RowPlus as usize].connect_port_tx(port_state)
    }
    pub fn connect_port_col_minus(&self, port_state: PortStateResult<T>) -> SimResult {
        self.arbiters[Port::ColMinus as usize].connect_port_tx(port_state)
    }
    pub fn connect_port_col_plus(&self, port_state: PortStateResult<T>) -> SimResult {
        self.arbiters[Port::ColPlus as usize].connect_port_tx(port_state)
    }

    pub fn port_row_minus(&self) -> PortStateResult<T> {
        self.routers[Port::RowMinus as usize].port_rx()
    }
    pub fn port_row_plus(&self) -> PortStateResult<T> {
        self.routers[Port::RowPlus as usize].port_rx()
    }
    pub fn port_col_minus(&self) -> PortStateResult<T> {
        self.routers[Port::ColMinus as usize].port_rx()
    }
    pub fn port_col_plus(&self) -> PortStateResult<T> {
        self.routers[Port::ColPlus as usize].port_rx()
    }
}
