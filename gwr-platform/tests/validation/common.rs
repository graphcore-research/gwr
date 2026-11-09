// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::compute::{
    CapabilityConfig, ComputeConfig, EngineConfig, OperationClass,
};
pub(crate) use gwr_platform::types::{
    CacheConfigSection, CacheSection, ConnectSection, FabricConfigSection, FabricKind,
    FabricSection, MemoryConfigSection, MemoryKind, MemoryMapSection, MemorySection,
    PlatformConfig, ProcessingElementConfigSection, ProcessingElementSection,
};

pub(crate) fn platform() -> PlatformConfig {
    PlatformConfig {
        coherency_managers: None,
        compute_profiles: Default::default(),
        operation_classes: basic_classes(),
        memory_maps: Vec::new(),
        defaults: None,
        processing_elements: None,
        caches: None,
        fabrics: None,
        memories: None,
        connections: None,
    }
}

pub(crate) fn basic_classes() -> BTreeMap<String, OperationClass> {
    ["add", "mul", "compare"]
        .into_iter()
        .map(|name| {
            (
                name.into(),
                OperationClass {
                    operations: vec![name.into()],
                    arity: 2,
                    flops_per_op: 1,
                },
            )
        })
        .collect()
}

pub(crate) fn memory(name: &str, base_address: u64, capacity_bytes: u64) -> MemorySection {
    MemorySection {
        name: name.to_string(),
        kind: MemoryKind::HBM,
        base_address,
        config: MemoryConfigSection {
            atomic_service: None,
            capacity_bytes,
            bw_bytes_per_tick: None,
            delay_ticks: None,
            arrival_aware_service: None,
        },
    }
}

pub(crate) fn processing_element(name: &str, memory_map: &str) -> ProcessingElementSection {
    ProcessingElementSection {
        name: name.to_string(),
        memory_map: memory_map.to_string(),
        dma_memory_map: None,
        config: ProcessingElementConfigSection {
            block_execution: None,
            num_active_requests: None,
            lsu_access_bytes: None,
            overhead_size_bytes: None,
            sram_bytes: None,
            boundary_payload_service: None,
            local_payload_service: None,
            compute: Some(gwr_platform::types::ComputeSelection::Inline(
                basic_compute(),
            )),
        },
    }
}

pub(crate) fn basic_compute() -> ComputeConfig {
    ComputeConfig {
        engines: BTreeMap::from([(
            "alu".into(),
            EngineConfig {
                matrix_accumulator_capacity_bytes: None,
                capabilities: ["add", "mul", "compare"]
                    .into_iter()
                    .map(|class| CapabilityConfig {
                        matrix_service_tile: None,
                        execution: None,
                        class: class.into(),
                        input_dtypes: None,
                        ops_per_tick: 16.0,
                    })
                    .collect(),
            },
        )]),
    }
}

pub(crate) fn memory_map(name: &str, devices: &[&str]) -> MemoryMapSection {
    MemoryMapSection {
        address_mapping: None,
        name: name.to_string(),
        devices: devices.iter().map(|name| (*name).to_string()).collect(),
    }
}

pub(crate) fn cache(name: &str) -> CacheSection {
    CacheSection {
        coherency_managers: None,
        name: name.to_string(),
        config: CacheConfigSection {
            max_inflight_parents: None,
            max_inflight_fills: None,
            bw_bytes_per_tick: None,
            line_size_bytes: None,
            num_ways: None,
            num_sets: None,
            delay_ticks: None,
        },
    }
}

pub(crate) fn fabric(name: &str) -> FabricSection {
    FabricSection {
        name: name.to_string(),
        kind: FabricKind::Functional,
        columns: 1,
        rows: 1,
        config: FabricConfigSection {
            fabric_ports_per_node: Some(2),
            ticks_per_hop: None,
            ticks_overhead: None,
            rx_buffer_bytes: None,
            tx_buffer_bytes: None,
            port_bits_per_tick: None,
            internal_link_bits_per_tick: None,
            routing: None,
            port_selection: None,
        },
    }
}

pub(crate) fn connection(from: &str, to: &str) -> ConnectSection {
    ConnectSection {
        connect: vec![from.to_string(), to.to_string()],
    }
}
