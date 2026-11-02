// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::processing_element::compute::{
    CapabilityConfig, CapabilityKind, ComputeConfig, ExecutionUnitConfig, OperationClass,
};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_platform::types::{
    AddressSpaceSection, CacheConfigSection, CacheSection, CoherencyManagerConfigSection,
    CoherencyManagerSection, ConnectSection, FabricConfigSection, FabricKind, FabricSection,
    MemoryConfigSection, MemoryKind, MemoryMapSection, MemorySection, PlatformConfig,
    ProcessingElementConfigSection, ProcessingElementSection,
};

pub(super) const PES: usize = 60;
pub(super) const BANKS: usize = 4;
pub(super) const BANK_CAPACITY: u64 = 256 * 1024 * 1024;

pub(super) fn endpoint_name(prefix: &str, index: usize) -> String {
    format!("{prefix}_{}_{}", index / 8, index % 8)
}

pub(super) fn bank_base(bank: usize) -> u64 {
    (u64::try_from(bank).unwrap() + 1) << 32
}

/// An 8x8 fabric with 60 PEs, coherent private 32KiB L1 caches, 64KiB SRAM
/// per PE and four HBM banks, each with its own directory.
pub(super) fn config() -> PlatformConfig {
    let pe_config = ProcessingElementConfigSection {
        num_active_requests: Some(8),
        sram_bytes: Some(64 * 1024),
        overhead_size_bytes: Some(8),
        compute: Some(compute_config()),
    };
    // 32 sets * 4 ways * 256-byte lines = 32KiB, private and coherent.
    let cache_config = CacheConfigSection {
        line_size_bytes: Some(256),
        num_sets: Some(32),
        num_ways: Some(4),
        bw_bytes_per_tick: Some(128),
        delay_ticks: Some(5),
        max_inflight_parents: Some(64),
        max_inflight_fills: Some(64),
    };
    PlatformConfig {
        operation_classes: BTreeMap::from([
            (
                "add".to_owned(),
                OperationClass {
                    operations: vec!["add".to_owned()],
                    arity: 2,
                    flops_per_op: 1,
                },
            ),
            (
                "matrix".to_owned(),
                OperationClass {
                    operations: vec!["matrix_mac".to_owned()],
                    arity: 2,
                    flops_per_op: 2,
                },
            ),
        ]),
        memory_maps: memory_maps(),
        address_spaces: vec![AddressSpaceSection {
            name: "default".to_owned(),
            memory_map: "default".to_owned(),
            access_bytes: Some(256),
            address_regions: None,
        }],
        defaults: None,
        processing_elements: Some(
            (0..PES)
                .map(|index| ProcessingElementSection {
                    name: endpoint_name("pe", index),
                    address_space: "default".to_owned(),
                    config: pe_config.clone(),
                })
                .collect(),
        ),
        caches: Some(
            (0..PES)
                .map(|index| CacheSection {
                    name: endpoint_name("l1", index),
                    coherency_managers: Some((0..BANKS).map(|bank| format!("cm{bank}")).collect()),
                    config: cache_config.clone(),
                })
                .collect(),
        ),
        coherency_managers: Some(coherency_managers()),
        fabrics: Some(vec![FabricSection {
            name: "interconnect".to_owned(),
            kind: FabricKind::Routed,
            columns: 8,
            rows: 8,
            config: FabricConfigSection {
                routing: Some(FabricRoutingAlgorithm::ColumnFirst),
                port_selection: None,
                fabric_ports_per_node: Some(2),
                ticks_per_hop: Some(2),
                ticks_overhead: Some(10),
                rx_buffer_bytes: Some(512),
                tx_buffer_bytes: Some(512),
                port_bits_per_tick: Some(1024),
                internal_link_bits_per_tick: Some(1024),
            },
        }]),
        memories: Some(
            (0..BANKS)
                .map(|bank| MemorySection {
                    name: format!("hbm{bank}"),
                    kind: MemoryKind::HBM,
                    base_address: bank_base(bank),
                    config: MemoryConfigSection {
                        capacity_bytes: BANK_CAPACITY,
                        bw_bytes_per_tick: Some(128),
                        delay_ticks: Some(10),
                    },
                })
                .collect(),
        ),
        connections: Some(platform_connections()),
    }
}

fn compute_config() -> ComputeConfig {
    let scalar = ExecutionUnitConfig {
        matrix_accumulator_capacity_bytes: None,
        capabilities: vec![CapabilityConfig {
            class: "add".to_owned(),
            input_dtypes: Some(vec![vec![DataType::Fp32; 2]]),
            ops_per_tick: 16.0,
            kind: CapabilityKind::Scalar,
        }],
    };
    let matrix = ExecutionUnitConfig {
        matrix_accumulator_capacity_bytes: None,
        capabilities: vec![CapabilityConfig {
            class: "matrix".to_owned(),
            input_dtypes: Some(vec![vec![DataType::Fp32; 2]]),
            // A MAC costs one multiply at 4/tick and one add at 16/tick.
            ops_per_tick: 1.0 / (1.0 / 4.0 + 1.0 / 16.0),
            kind: CapabilityKind::Matrix {
                accumulator: DataType::Fp32,
                matrix_service_tile: None,
            },
        }],
    };
    ComputeConfig {
        execution_units: BTreeMap::from([
            ("scalar".to_owned(), scalar),
            ("matrix".to_owned(), matrix),
        ]),
    }
}

fn memory_maps() -> Vec<MemoryMapSection> {
    let mut maps = vec![MemoryMapSection {
        name: "default".to_owned(),
        devices: (0..BANKS).map(|bank| format!("hbm{bank}")).collect(),
    }];
    maps.extend((0..BANKS).map(|bank| MemoryMapSection {
        name: format!("coherent_bank{bank}"),
        devices: vec![format!("hbm{bank}")],
    }));
    maps
}

fn coherency_managers() -> Vec<CoherencyManagerSection> {
    (0..BANKS)
        .map(|bank| CoherencyManagerSection {
            name: format!("cm{bank}"),
            memory_map: format!("coherent_bank{bank}"),
            config: CoherencyManagerConfigSection {
                line_size_bytes: Some(256),
                operations_per_tick: None,
            },
        })
        .collect()
}

fn platform_connections() -> Vec<ConnectSection> {
    let mut connections = Vec::with_capacity(3 * PES + 2 * BANKS);
    for index in 0..PES {
        let pe = endpoint_name("pe", index);
        let l1 = endpoint_name("l1", index);
        let (column, row) = (index / 8, index % 8);
        connections.push(ConnectSection {
            connect: vec![format!("pe.{pe}"), format!("cache.{l1}.dev")],
        });
        connections.push(ConnectSection {
            connect: vec![
                "null".to_owned(),
                format!("fabric.interconnect@({column},{row}).1"),
            ],
        });
        connections.push(ConnectSection {
            connect: vec![
                format!("cache.{l1}.mem"),
                format!("fabric.interconnect@({column},{row}).0"),
            ],
        });
    }
    for bank in 0..BANKS {
        let row = bank + 4;
        connections.push(ConnectSection {
            connect: vec![
                format!("mem.hbm{bank}"),
                format!("fabric.interconnect@(7,{row}).0"),
            ],
        });
        connections.push(ConnectSection {
            connect: vec![
                format!("coherency_manager.cm{bank}"),
                format!("fabric.interconnect@(7,{row}).1"),
            ],
        });
    }
    connections
}
