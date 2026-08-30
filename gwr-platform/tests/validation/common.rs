// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

pub(crate) use gwr_engine::test_helpers::start_test;
pub(crate) use gwr_platform::Platform;
pub(crate) use gwr_platform::types::{
    CacheConfigSection, CacheSection, ConnectSection, FabricConfigSection, FabricKind,
    FabricSection, MemoryConfigSection, MemoryDeviceSection, MemoryKind, MemoryMapSection,
    MemorySection, PlatformConfig, ProcessingElementConfigSection, ProcessingElementSection,
};

pub(crate) fn platform() -> PlatformConfig {
    PlatformConfig {
        memory_maps: Vec::new(),
        defaults: None,
        processing_elements: None,
        caches: None,
        fabrics: None,
        memories: None,
        connections: None,
    }
}

pub(crate) fn memory(name: &str, base_address: u64, capacity_bytes: u64) -> MemorySection {
    MemorySection {
        name: name.to_string(),
        kind: MemoryKind::HBM,
        base_address,
        config: MemoryConfigSection {
            capacity_bytes,
            bw_bytes_per_tick: None,
            delay_ticks: None,
        },
    }
}

pub(crate) fn memory_map(name: &str, devices: &[&str]) -> MemoryMapSection {
    MemoryMapSection {
        name: name.to_string(),
        devices: devices
            .iter()
            .map(|name| MemoryDeviceSection {
                name: (*name).to_string(),
            })
            .collect(),
    }
}

pub(crate) fn processing_element(name: &str, memory_map: &str) -> ProcessingElementSection {
    ProcessingElementSection {
        name: name.to_string(),
        memory_map: memory_map.to_string(),
        config: ProcessingElementConfigSection {
            num_active_requests: None,
            lsu_access_bytes: None,
            overhead_size_bytes: None,
            sram_bytes: None,
            adds_per_tick: None,
            muls_per_tick: None,
            compares_per_tick: None,
        },
    }
}

pub(crate) fn cache(name: &str) -> CacheSection {
    CacheSection {
        name: name.to_string(),
        config: CacheConfigSection {
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
            routing: None,
        },
    }
}

pub(crate) fn connection(from: &str, to: &str) -> ConnectSection {
    ConnectSection {
        connect: vec![from.to_string(), to.to_string()],
    }
}
