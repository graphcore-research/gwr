// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Models of fabric interconnects.
//!
//! For simplicity, fabrics are assumed to be rectangular (N columns x M rows)
//! collections of nodes with each node allocated P ingress/egress port IDs.
//! However, if the user limits the number of ports per node then not all
//! ingress/egress ports will be populated.

use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display};

use clap::ValueEnum;
use gwr_engine::port::PortStateResult;
use gwr_engine::sim_error;
use gwr_engine::traits::{Routable, SimObject};
use gwr_engine::types::{SimError, SimResult};
use gwr_track::entity::GetEntity;
use rand::{Rng, SeedableRng};
use rand_xoshiro::SplitMix64;
use serde::{Deserialize, Serialize};

pub trait Fabric<T>: GetEntity + Display
where
    T: SimObject + Routable,
{
    fn connect_port_egress_i(&self, i: usize, port_state: PortStateResult<T>) -> SimResult;
    fn port_ingress_i(&self, i: usize) -> PortStateResult<T>;
    fn col_row_port_to_fabric_port_index(&self, col: usize, row: usize, port: usize) -> usize;
    fn fabric_port_index_to_col_row_port(&self, fabric_port_index: usize) -> (usize, usize, usize);
    fn destination_port_map(&self) -> &HashMap<u64, Vec<usize>>;
    fn port_selection(&self) -> FabricPortSelection;
}

pub enum RoutingAlgoritm {
    ColumnFirst,
    RowFirst,
}

#[derive(ValueEnum, Clone, Copy, Default, Debug, Serialize, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FabricPortSelection {
    #[default]
    DestinationAddressHash,
    SourceIdModulo,
}

impl fmt::Display for FabricPortSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            FabricPortSelection::DestinationAddressHash => "destination-address-hash",
            FabricPortSelection::SourceIdModulo => "source-id-modulo",
        };
        f.write_str(s)
    }
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

    /// Number of bytes in the rx buffer for each fabric port
    rx_buffer_bytes: usize,

    /// Number of bytes in the tx buffer for each fabric port
    tx_buffer_bytes: usize,

    /// Set the throughput limit on each port (in bits per tick)
    port_bits_per_tick: usize,

    /// Indices of populated ingress/egress ports
    fabric_port_indices: Vec<usize>,

    /// Mapping from protocol destinations, such as device IDs, to fabric
    /// egress port indices.
    destination_port_map: HashMap<u64, Vec<usize>>,

    /// Policy used when a destination maps to multiple egress ports.
    port_selection: FabricPortSelection,
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

fn validate_destination_port_map(
    destination_port_map: &HashMap<u64, Vec<usize>>,
    fabric_port_indices: &[usize],
) -> Result<(), SimError> {
    let fabric_port_indices: HashSet<usize> = fabric_port_indices.iter().copied().collect();
    for (destination, ports) in destination_port_map {
        for port in ports {
            if !fabric_port_indices.contains(port) {
                return sim_error!(
                    "Destination port map references unpopulated fabric port {port} for destination {destination}"
                );
            }
        }
    }
    Ok(())
}

impl FabricConfig {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        num_columns: usize,
        num_rows: usize,
        num_ports_per_node: usize,
        ports_per_node_limit: Option<usize>,
        cycles_per_hop: usize,
        cycles_overhead: usize,
        rx_buffer_bytes: usize,
        tx_buffer_bytes: usize,
        port_bits_per_tick: usize,
        destination_port_map: HashMap<u64, Vec<usize>>,
    ) -> Result<Self, SimError> {
        let fabric_port_indices = create_populated_indices(
            num_columns,
            num_rows,
            num_ports_per_node,
            ports_per_node_limit,
        );
        validate_destination_port_map(&destination_port_map, &fabric_port_indices)?;
        Ok(Self {
            num_columns,
            num_rows,
            num_ports_per_node,
            ports_per_node_limit,
            cycles_per_hop,
            cycles_overhead,
            rx_buffer_bytes,
            tx_buffer_bytes,
            port_bits_per_tick,
            fabric_port_indices,
            destination_port_map,
            port_selection: FabricPortSelection::default(),
        })
    }

    #[must_use]
    pub fn with_port_selection(mut self, port_selection: FabricPortSelection) -> Self {
        self.port_selection = port_selection;
        self
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

    #[must_use]
    pub fn destination_port_map(&self) -> &HashMap<u64, Vec<usize>> {
        &self.destination_port_map
    }

    #[must_use]
    pub fn port_selection(&self) -> FabricPortSelection {
        self.port_selection
    }

    pub fn resolve_destination_port<T>(&self, object: &T) -> Result<usize, SimError>
    where
        T: Routable,
    {
        let destination = object.dst_device().0;
        let ports = self.destination_port_map.get(&destination).ok_or_else(|| {
            SimError(format!(
                "No fabric egress port mapped for destination {destination}"
            ))
        })?;

        match ports.as_slice() {
            [] => Err(SimError(format!(
                "No fabric egress port mapped for destination {destination}"
            ))),
            [port] => Ok(*port),
            _ => {
                let selector = match self.port_selection {
                    FabricPortSelection::DestinationAddressHash => splitmix64(object.dst_addr()),
                    FabricPortSelection::SourceIdModulo => object.src_device().0,
                };
                Ok(ports[(selector as usize) % ports.len()])
            }
        }
    }
}

#[must_use]
fn splitmix64(seed: u64) -> u64 {
    let mut rng = SplitMix64::seed_from_u64(seed);
    rng.next_u64()
}

pub mod functional;
pub mod node;
pub mod routed;

#[test]
fn port_index() {
    let config: FabricConfig =
        FabricConfig::new(3, 4, 2, None, 1, 1, 1, 1, 1, HashMap::new()).unwrap();

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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gwr_engine::traits::Routable;
    use gwr_engine::types::{AccessType, DeviceId};

    use super::{FabricConfig, FabricPortSelection, splitmix64};

    const MULTI_ROUTE_PORTS: [usize; 3] = [10, 11, 12];

    struct TestRoutable {
        dst_device: DeviceId,
        src_device: DeviceId,
        dst_addr: u64,
        src_addr: u64,
    }

    impl Routable for TestRoutable {
        fn dst_addr(&self) -> u64 {
            self.dst_addr
        }

        fn src_addr(&self) -> u64 {
            self.src_addr
        }

        fn dst_device(&self) -> DeviceId {
            self.dst_device
        }

        fn src_device(&self) -> DeviceId {
            self.src_device
        }

        fn access_type(&self) -> AccessType {
            AccessType::Control
        }
    }

    fn config_with_map(selection: FabricPortSelection) -> FabricConfig {
        FabricConfig::new(
            4,
            1,
            4,
            None,
            1,
            1,
            1,
            1,
            1,
            HashMap::from([(7, MULTI_ROUTE_PORTS.to_vec())]),
        )
        .unwrap()
        .with_port_selection(selection)
    }

    #[test]
    fn resolves_single_destination_port() {
        let config =
            FabricConfig::new(2, 2, 1, None, 1, 1, 1, 1, 1, HashMap::from([(7, vec![3])])).unwrap();
        let packet = TestRoutable {
            dst_device: DeviceId(7),
            src_device: DeviceId(1),
            dst_addr: 2,
            src_addr: 1,
        };

        assert_eq!(config.resolve_destination_port(&packet).unwrap(), 3);
    }

    #[test]
    fn errors_for_missing_destination_port() {
        let config = config_with_map(FabricPortSelection::DestinationAddressHash);
        let packet = TestRoutable {
            dst_device: DeviceId(8),
            src_device: DeviceId(1),
            dst_addr: 2,
            src_addr: 1,
        };

        assert!(
            format!("{}", config.resolve_destination_port(&packet).unwrap_err())
                .contains("No fabric egress port mapped for destination 8")
        );
    }

    #[test]
    fn errors_for_empty_destination_port_map() {
        let config = FabricConfig::new(2, 1, 1, None, 1, 1, 1, 1, 1, HashMap::new()).unwrap();
        let packet = TestRoutable {
            dst_device: DeviceId(0),
            src_device: DeviceId(1),
            dst_addr: 2,
            src_addr: 1,
        };

        assert!(
            format!("{}", config.resolve_destination_port(&packet).unwrap_err())
                .contains("No fabric egress port mapped for destination 0")
        );
    }

    #[test]
    fn rejects_destination_port_map_with_out_of_range_port() {
        let error =
            match FabricConfig::new(2, 1, 1, None, 1, 1, 1, 1, 1, HashMap::from([(7, vec![2])])) {
                Ok(_) => panic!("expected invalid destination port map to return an error"),
                Err(error) => error,
            };

        assert!(error.to_string().contains(
            "Destination port map references unpopulated fabric port 2 for destination 7"
        ));
    }

    #[test]
    fn rejects_destination_port_map_with_unpopulated_port() {
        let error = match FabricConfig::new(
            2,
            2,
            2,
            Some(3),
            1,
            1,
            1,
            1,
            1,
            HashMap::from([(7, vec![1])]),
        ) {
            Ok(_) => panic!("expected invalid destination port map to return an error"),
            Err(error) => error,
        };

        assert!(error.to_string().contains(
            "Destination port map references unpopulated fabric port 1 for destination 7"
        ));
    }

    #[test]
    fn resolves_multi_route_by_destination_address_hash() {
        let config = config_with_map(FabricPortSelection::DestinationAddressHash);
        let packet = TestRoutable {
            dst_device: DeviceId(7),
            src_device: DeviceId(4),
            dst_addr: 5,
            src_addr: 4,
        };
        let expected = MULTI_ROUTE_PORTS[(splitmix64(5) as usize) % MULTI_ROUTE_PORTS.len()];

        assert_eq!(config.resolve_destination_port(&packet).unwrap(), expected);
    }

    #[test]
    fn resolves_multi_route_by_source_id_modulo() {
        let config = config_with_map(FabricPortSelection::SourceIdModulo);
        let packet = TestRoutable {
            dst_device: DeviceId(7),
            src_device: DeviceId(4),
            dst_addr: 5,
            src_addr: 4,
        };
        let expected = MULTI_ROUTE_PORTS[(packet.src_device.0 as usize) % MULTI_ROUTE_PORTS.len()];

        assert_eq!(config.resolve_destination_port(&packet).unwrap(), expected);
    }
}
