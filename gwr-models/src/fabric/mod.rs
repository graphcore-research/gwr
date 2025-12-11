// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Models of fabric interconnects.
//!
//! For simplicity, fabrics are assumed to be rectangular (N columns x M rows)
//! collections of nodes with each node allocated P ingress/egress port IDs.
//! However, if the user limits the number of ports per node then not all
//! ingress/egress ports will be populated.

use std::cmp::min;
use std::fmt::Display;

use gwr_engine::port::PortStateResult;
use gwr_engine::traits::{Routable, SimObject};
use gwr_engine::types::SimResult;
use gwr_track::entity::GetEntity;

pub trait Fabric<T>: GetEntity + Display
where
    T: SimObject + Routable,
{
    fn connect_port_egress_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult;
    fn port_ingress_i(&self, i: usize) -> PortStateResult<T>;
    fn col_row_port_to_fabric_port_index(&self, col: usize, row: usize, port: usize) -> usize;
}

pub enum RoutingAlgoritm {
    ColumnFirst,
    RowFirst,
}

/// Configuration structure for a fabric
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

    /// Cycles per hop when routing between an ingress and egress port
    cycles_per_hop: usize,

    /// Fixed overhead to be added to routing delay
    cycles_overhead: usize,

    /// Number of entries in the rx buffer for each fabric port
    rx_buffer_entries: usize,

    /// Number of entries in the tx buffer for each fabric port
    tx_buffer_entries: usize,

    /// Set the throughput limit on each port (in bits per tick)
    port_bits_per_tick: usize,

    /// Indices of populated ingress/egress ports
    fabric_port_indices: Vec<usize>,
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
/// Given a col/row position of a node in a fabric, compute how many
/// ingress/egress ports there are
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

impl FabricConfig {
    #[expect(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        num_columns: usize,
        num_rows: usize,
        num_ports_per_node: usize,
        ports_per_node_limit: Option<usize>,
        cycles_per_hop: usize,
        cycles_overhead: usize,
        rx_buffer_entries: usize,
        tx_buffer_entries: usize,
        port_bits_per_tick: usize,
    ) -> Self {
        let fabric_port_indices = create_populated_indices(
            num_columns,
            num_rows,
            num_ports_per_node,
            ports_per_node_limit,
        );
        Self {
            num_columns,
            num_rows,
            num_ports_per_node,
            ports_per_node_limit,
            cycles_per_hop,
            cycles_overhead,
            rx_buffer_entries,
            tx_buffer_entries,
            port_bits_per_tick,
            fabric_port_indices,
        }
    }

    /// Returns the maximum number of ports in the fabric
    #[must_use]
    pub fn max_num_ports(&self) -> usize {
        self.num_columns * self.num_rows * self.num_ports_per_node
    }

    /// Returns the number of ports in a fabric.
    #[must_use]
    pub fn num_ports(&self) -> usize {
        self.fabric_port_indices.len()
    }

    /// Returns the actual port indices
    #[must_use]
    pub fn port_indices(&self) -> &Vec<usize> {
        &self.fabric_port_indices
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
    pub fn cycles_per_hop(&self) -> usize {
        self.cycles_per_hop
    }

    #[must_use]
    pub fn cycles_overhead(&self) -> usize {
        self.cycles_overhead
    }

    #[must_use]
    pub fn port_bits_per_tick(&self) -> usize {
        self.port_bits_per_tick
    }
}

pub mod functional;
pub mod node;
pub mod routed;

#[test]
fn port_index() {
    let config: FabricConfig = FabricConfig::new(3, 4, 2, None, 1, 1, 1, 1, 1);

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
