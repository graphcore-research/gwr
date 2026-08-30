// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Models of fabric interconnects.
//!
//! For simplicity, fabrics are assumed to be rectangular (N columns x M rows)
//! collections of nodes with each node allocated P ingress/egress port IDs.
//! However, if the user limits the number of ports per node then not all
//! ingress/egress ports will be populated.

use std::cell::OnceCell;
use std::cmp::min;
use std::fmt::Display;

use gwr_engine::port::PortStateResult;
use gwr_engine::sim_error;
use gwr_engine::traits::{Routable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_track::entity::GetEntity;

pub trait Fabric<T>: GetEntity + Display
where
    T: SimObject + Routable,
{
    fn connect_port_egress_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult;
    fn port_ingress_i(&self, i: usize) -> PortStateResult<T>;
    fn col_row_port_to_fabric_port_index(&self, col: usize, row: usize, port: usize) -> usize;
}

/// Configuration structure for a fabric
#[derive(Debug)]
pub struct FabricConfig {
    /// Number of columns in the fabric
    num_columns: usize,

    /// Number of rows in the fabric
    num_rows: usize,

    /// Number of ingress/egress port pairs at each node of the fabric
    num_ports_per_node: usize,

    /// Optional limit to total number of ports on a node. Depending on
    /// where in the fabric a node is there will be up to 4 internal ports
    /// already used for x/y routing.
    ports_per_node_limit: Option<usize>,

    /// Ticks per hop when routing between an ingress and egress port
    ticks_per_hop: usize,

    /// Fixed overhead to be added to routing delay
    ticks_overhead: usize,

    /// Number of bytes in the rx buffer for each fabric port
    rx_buffer_bytes: usize,

    /// Number of bytes in the tx buffer for each fabric port
    tx_buffer_bytes: usize,

    /// Set the throughput limit on each port (in bits per tick)
    port_bits_per_tick: usize,

    /// Number of populated ingress/egress ports.
    num_ports: usize,

    /// Indices of populated ingress/egress ports, created only when requested.
    fabric_port_indices: OnceCell<Vec<usize>>,

    max_num_ports: usize,
}

/// Fabric dimensions and externally available ports.
#[derive(Clone, Copy, Debug)]
pub struct FabricGeometry {
    /// Number of columns.
    pub num_columns: usize,
    /// Number of rows.
    pub num_rows: usize,
    /// Number of externally available port pairs on each node.
    pub num_ports_per_node: usize,
    /// Optional limit including the ports used for internal routing.
    pub ports_per_node_limit: Option<usize>,
}

/// Transfer timing and buffering for each fabric port.
#[derive(Clone, Copy, Debug)]
pub struct FabricPortConfig {
    /// Ticks added for each routed hop.
    pub ticks_per_hop: usize,
    /// Fixed ticks added to each transfer.
    pub ticks_overhead: usize,
    /// Receive-buffer capacity for each port.
    pub rx_buffer_bytes: usize,
    /// Transmit-buffer capacity for each port.
    pub tx_buffer_bytes: usize,
    /// Transfer rate for each port.
    pub port_bits_per_tick: usize,
}

impl FabricConfig {
    pub fn new(geometry: FabricGeometry, ports: FabricPortConfig) -> Result<Self, SimError> {
        let FabricGeometry {
            num_columns,
            num_rows,
            num_ports_per_node,
            ports_per_node_limit,
        } = geometry;
        let FabricPortConfig {
            ticks_per_hop,
            ticks_overhead,
            rx_buffer_bytes,
            tx_buffer_bytes,
            port_bits_per_tick,
        } = ports;
        let max_num_ports = num_columns
            .checked_mul(num_rows)
            .and_then(|nodes| nodes.checked_mul(num_ports_per_node))
            .ok_or_else(|| SimError("maximum port count overflows".to_string()))?;
        let num_ports = populated_port_count(
            num_columns,
            num_rows,
            num_ports_per_node,
            ports_per_node_limit,
        )
        .ok_or_else(|| SimError("populated port count overflows".to_string()))?;
        if num_ports < 2 {
            let noun = if num_ports == 1 { "port" } else { "ports" };
            return sim_error!("has {num_ports} populated {noun}; at least 2 are required");
        }
        if rx_buffer_bytes == 0 {
            return sim_error!("receive buffer size must be greater than zero");
        }
        if tx_buffer_bytes == 0 {
            return sim_error!("transmit buffer size must be greater than zero");
        }
        if port_bits_per_tick == 0 {
            return sim_error!("link rate must be greater than zero");
        }

        Ok(Self {
            num_columns,
            num_rows,
            num_ports_per_node,
            ports_per_node_limit,
            ticks_per_hop,
            ticks_overhead,
            rx_buffer_bytes,
            tx_buffer_bytes,
            port_bits_per_tick,
            num_ports,
            fabric_port_indices: OnceCell::new(),
            max_num_ports,
        })
    }

    /// Returns the maximum number of ports in the fabric
    #[must_use]
    pub fn max_num_ports(&self) -> usize {
        self.max_num_ports
    }

    /// Returns the number of ports in a fabric.
    #[must_use]
    pub fn num_ports(&self) -> usize {
        self.num_ports
    }

    /// Returns the actual port indices
    #[must_use]
    pub fn port_indices(&self) -> &[usize] {
        self.fabric_port_indices.get_or_init(|| {
            create_populated_indices(
                self.num_columns,
                self.num_rows,
                self.num_ports_per_node,
                self.ports_per_node_limit,
            )
        })
    }

    /// Given a column, row and port index, return the overall index in the
    /// fabric ports
    ///
    /// Ports laid out as
    /// ports\[col\]\[row\]\[port\]
    #[must_use]
    pub fn col_row_port_to_fabric_port_index(&self, col: usize, row: usize, port: usize) -> usize {
        col_row_port_to_fabric_port_index(self.num_rows, self.num_ports_per_node, col, row, port)
    }

    #[must_use]
    pub fn fabric_port_index_to_col_row_port(
        &self,
        fabric_port_index: usize,
    ) -> (usize, usize, usize) {
        let col = fabric_port_index / self.num_ports_per_node / self.num_rows;
        let row = (fabric_port_index / self.num_ports_per_node) % self.num_rows;
        let port = fabric_port_index % self.num_ports_per_node;
        (col, row, port)
    }

    #[must_use]
    pub fn node_num_ingress_egress_ports(&self, col: usize, row: usize) -> usize {
        node_num_ingress_egress_ports(
            self.num_columns,
            self.num_rows,
            self.num_ports_per_node,
            self.ports_per_node_limit,
            col,
            row,
        )
    }

    #[must_use]
    pub fn max_x(&self) -> usize {
        self.num_columns - 1
    }

    #[must_use]
    pub fn max_y(&self) -> usize {
        self.num_rows - 1
    }

    #[must_use]
    pub fn num_columns(&self) -> usize {
        self.num_columns
    }

    #[must_use]
    pub fn num_rows(&self) -> usize {
        self.num_rows
    }

    #[must_use]
    pub fn num_ports_per_node(&self) -> usize {
        self.num_ports_per_node
    }

    #[must_use]
    pub fn ticks_per_hop(&self) -> usize {
        self.ticks_per_hop
    }

    #[must_use]
    pub fn ticks_overhead(&self) -> usize {
        self.ticks_overhead
    }

    #[must_use]
    pub fn port_bits_per_tick(&self) -> usize {
        self.port_bits_per_tick
    }
}

fn populated_port_count(
    num_columns: usize,
    num_rows: usize,
    num_ports_per_node: usize,
    ports_per_node_limit: Option<usize>,
) -> Option<usize> {
    let Some(limit) = ports_per_node_limit else {
        return num_columns
            .checked_mul(num_rows)
            .and_then(|nodes| nodes.checked_mul(num_ports_per_node));
    };

    let edge_columns = num_columns.min(2);
    let edge_rows = num_rows.min(2);
    let inner_columns = num_columns - edge_columns;
    let inner_rows = num_rows - edge_rows;
    // Nodes whose column and row are on a boundary use two routing ports.
    // Nodes on one boundary use three, and interior nodes use four. Grouping
    // them here avoids visiting every node merely to count the externally
    // available ports.
    let node_groups = [
        (edge_columns.checked_mul(edge_rows)?, 2usize),
        (
            edge_columns
                .checked_mul(inner_rows)?
                .checked_add(inner_columns.checked_mul(edge_rows)?)?,
            3,
        ),
        (inner_columns.checked_mul(inner_rows)?, 4),
    ];

    node_groups
        .into_iter()
        .try_fold(0usize, |total, (nodes, routing_ports)| {
            let ports = min(limit.saturating_sub(routing_ports), num_ports_per_node);
            total.checked_add(nodes.checked_mul(ports)?)
        })
}

fn create_populated_indices(
    num_columns: usize,
    num_rows: usize,
    num_ports_per_node: usize,
    ports_per_node_limit: Option<usize>,
) -> Vec<usize> {
    let mut fabric_indices = Vec::new();
    for col in 0..num_columns {
        for row in 0..num_rows {
            let num_ports = node_num_ingress_egress_ports(
                num_columns,
                num_rows,
                num_ports_per_node,
                ports_per_node_limit,
                col,
                row,
            );
            for port in 0..num_ports {
                fabric_indices.push(col_row_port_to_fabric_port_index(
                    num_rows,
                    num_ports_per_node,
                    col,
                    row,
                    port,
                ));
            }
        }
    }
    fabric_indices
}

/// Given a col/row position of a node in a fabric, compute how many
/// ingress/egress ports there are
#[must_use]
fn node_num_ingress_egress_ports(
    num_columns: usize,
    num_rows: usize,
    num_ports_per_node: usize,
    ports_per_node_limit: Option<usize>,
    col: usize,
    row: usize,
) -> usize {
    match ports_per_node_limit {
        None => num_ports_per_node,
        Some(ports_per_node_limit) => {
            let num_x_y_ports = num_x_y_ports(num_columns, num_rows, col, row);
            let max_ingress_egress_ports = ports_per_node_limit.saturating_sub(num_x_y_ports);
            min(max_ingress_egress_ports, num_ports_per_node)
        }
    }
}

#[must_use]
fn num_x_y_ports(num_columns: usize, num_rows: usize, col: usize, row: usize) -> usize {
    let mut num_ports = 4;
    if col == 0 || col == num_columns - 1 {
        // Left/right edge
        num_ports -= 1;
    }
    if row == 0 || row == num_rows - 1 {
        // Top/bottom edge
        num_ports -= 1;
    }
    num_ports
}

#[must_use]
fn col_row_port_to_fabric_port_index(
    num_rows: usize,
    num_ports_per_node: usize,
    col: usize,
    row: usize,
    port: usize,
) -> usize {
    port + row * num_ports_per_node + col * num_rows * num_ports_per_node
}

pub mod functional;
pub mod node;
pub mod routed;

#[test]
fn port_index() {
    let config = FabricConfig::new(
        FabricGeometry {
            num_columns: 3,
            num_rows: 4,
            num_ports_per_node: 2,
            ports_per_node_limit: None,
        },
        FabricPortConfig {
            ticks_per_hop: 1,
            ticks_overhead: 1,
            rx_buffer_bytes: 1,
            tx_buffer_bytes: 1,
            port_bits_per_tick: 1,
        },
    )
    .unwrap();

    assert_eq!(config.col_row_port_to_fabric_port_index(0, 0, 0), 0);
    assert_eq!(config.fabric_port_index_to_col_row_port(0), (0, 0, 0));

    assert_eq!(config.col_row_port_to_fabric_port_index(0, 0, 1), 1);
    assert_eq!(config.fabric_port_index_to_col_row_port(1), (0, 0, 1));

    assert_eq!(config.col_row_port_to_fabric_port_index(0, 1, 0), 2);
    assert_eq!(config.fabric_port_index_to_col_row_port(2), (0, 1, 0));

    assert_eq!(config.col_row_port_to_fabric_port_index(0, 1, 1), 3);
    assert_eq!(config.fabric_port_index_to_col_row_port(3), (0, 1, 1));

    assert_eq!(config.col_row_port_to_fabric_port_index(1, 0, 0), 8);
    assert_eq!(config.fabric_port_index_to_col_row_port(8), (1, 0, 0));

    assert_eq!(config.col_row_port_to_fabric_port_index(1, 3, 0), 14);
    assert_eq!(config.fabric_port_index_to_col_row_port(14), (1, 3, 0));

    assert_eq!(config.col_row_port_to_fabric_port_index(2, 1, 1), 19);
    assert_eq!(config.fabric_port_index_to_col_row_port(19), (2, 1, 1));
}

#[test]
fn counts_populated_ports_without_materialising_indices() {
    let config = FabricConfig::new(
        FabricGeometry {
            num_columns: 1_000_000_000,
            num_rows: 1,
            num_ports_per_node: 1,
            ports_per_node_limit: None,
        },
        FabricPortConfig {
            ticks_per_hop: 0,
            ticks_overhead: 0,
            rx_buffer_bytes: 1,
            tx_buffer_bytes: 1,
            port_bits_per_tick: 1,
        },
    )
    .unwrap();

    assert_eq!(config.num_ports(), 1_000_000_000);
    assert!(config.fabric_port_indices.get().is_none());
}

#[test]
fn populated_port_indices_match_count_and_bounds() {
    for num_columns in 1..=4 {
        for num_rows in 1..=4 {
            for num_ports_per_node in 1..=4 {
                for ports_per_node_limit in [None, Some(2), Some(3), Some(4), Some(5), Some(6)] {
                    let result = FabricConfig::new(
                        FabricGeometry {
                            num_columns,
                            num_rows,
                            num_ports_per_node,
                            ports_per_node_limit,
                        },
                        FabricPortConfig {
                            ticks_per_hop: 0,
                            ticks_overhead: 0,
                            rx_buffer_bytes: 1,
                            tx_buffer_bytes: 1,
                            port_bits_per_tick: 1,
                        },
                    );
                    let Ok(config) = result else {
                        continue;
                    };

                    let port_indices = config.port_indices();
                    let unique_indices: std::collections::HashSet<_> =
                        port_indices.iter().copied().collect();

                    assert_eq!(config.num_ports(), port_indices.len());
                    assert_eq!(unique_indices.len(), port_indices.len());
                    assert!(
                        port_indices
                            .iter()
                            .all(|&index| index < config.max_num_ports())
                    );
                }
            }
        }
    }
}
