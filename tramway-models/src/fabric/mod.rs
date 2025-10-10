// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Models of fabric interconnects.

/// Configuration structure for a fabric
pub struct FabricConfig {
    /// Number of columns in the fabric
    num_columns: usize,

    /// Number of rows in the fabric
    num_rows: usize,

    /// Number of rx/tx port pairs at each node of the fabric
    num_ports_per_node: usize,

    /// Cycles per hop when routing between an RX and TX port
    cycles_per_hop: usize,

    /// Fixed overhead to be added to routing delay
    cycles_overhead: usize,

    /// Number of entries in the rx buffer for each fabric port
    rx_buffer_entries: usize,

    /// Number of entries in the tx buffer for each fabric port
    tx_buffer_entries: usize,

    /// Set the throughput limit on each port (in bits per tick)
    port_bits_per_tick: usize,
}

impl FabricConfig {
    #[expect(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        num_columns: usize,
        num_rows: usize,
        num_ports_per_node: usize,
        cycles_per_hop: usize,
        cycles_overhead: usize,
        rx_buffer_entries: usize,
        tx_buffer_entries: usize,
        port_bits_per_tick: usize,
    ) -> Self {
        Self {
            num_columns,
            num_rows,
            num_ports_per_node,
            cycles_per_hop,
            cycles_overhead,
            rx_buffer_entries,
            tx_buffer_entries,
            port_bits_per_tick,
        }
    }

    /// Returns the total number of ports in a fabric.
    #[must_use]
    pub fn num_ports(&self) -> usize {
        self.num_columns * self.num_rows * self.num_ports_per_node
    }

    /// Given a column, row and node port index, return the overall index in the
    /// fabric ports
    ///
    /// Ports laid out as
    /// ports\[col\]\[row\]\[node_index\]
    #[must_use]
    pub fn port_index(&self, col: usize, row: usize, node_index: usize) -> usize {
        node_index + row * self.num_ports_per_node + col * self.num_rows * self.num_ports_per_node
    }

    #[must_use]
    pub fn port_col_row_index(&self, port_index: usize) -> (usize, usize, usize) {
        let col = port_index / self.num_ports_per_node / self.num_rows;
        let row = (port_index / self.num_ports_per_node) % self.num_rows;
        let node_index = port_index % self.num_ports_per_node;
        (col, row, node_index)
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

#[test]
fn port_index() {
    let config: FabricConfig = FabricConfig::new(3, 4, 2, 1, 1, 1, 1, 1);

    assert_eq!(config.port_index(0, 0, 0), 0);
    assert_eq!(config.port_col_row_index(0), (0, 0, 0));

    assert_eq!(config.port_index(0, 0, 1), 1);
    assert_eq!(config.port_col_row_index(1), (0, 0, 1));

    assert_eq!(config.port_index(0, 1, 0), 2);
    assert_eq!(config.port_col_row_index(2), (0, 1, 0));

    assert_eq!(config.port_index(0, 1, 1), 3);
    assert_eq!(config.port_col_row_index(3), (0, 1, 1));

    assert_eq!(config.port_index(1, 0, 0), 8);
    assert_eq!(config.port_col_row_index(8), (1, 0, 0));

    assert_eq!(config.port_index(1, 3, 0), 14);
    assert_eq!(config.port_col_row_index(14), (1, 3, 0));

    assert_eq!(config.port_index(2, 1, 1), 19);
    assert_eq!(config.port_col_row_index(19), (2, 1, 1));
}
