// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimError;
use gwr_models::fabric::functional::FunctionalFabric;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::fabric::routed::RoutedFabric;
use gwr_models::fabric::{Fabric, FabricConfig};
use gwr_models::memory::cache::{Cache, CacheConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::processing_element::{ProcessingElement, ProcessingElementConfig};
use gwr_track::entity::{Entity, GetEntity};

use crate::types::{FabricKind, MemoryMapSection, PlatformConfig, ProcessingElementConfigSection};
use crate::{Caches, DeviceIds, Fabrics, Memories, NameToIdxMap, ProcessingElements};

pub fn build_memory_map(
    cfg: &MemoryMapSection,
    memories: &Memories,
    memories_idx_by_id: &NameToIdxMap,
    device_ids: &DeviceIds,
) -> Result<MemoryMap, SimError> {
    let mut memory_map = MemoryMap::new();
    for device in &cfg.devices {
        let memory_idx = memories_idx_by_id
            .get(device.name.as_str())
            .ok_or_else(|| SimError(format!("Unknown memory '{}'", device.name)))?;
        let memory = &memories[*memory_idx];
        let device_id = *device_ids
            .get(&device.name)
            .ok_or_else(|| SimError(format!("Unknown device '{}'", device.name)))?;
        memory_map.insert(
            memory.base_address(),
            memory.capacity_bytes() as u64,
            device_id,
        )?;
    }
    Ok(memory_map)
}

pub fn build_memory_maps(
    cfg: &PlatformConfig,
    memories: &Memories,
    memories_idx_by_id: &NameToIdxMap,
    device_ids: &DeviceIds,
) -> Result<HashMap<String, Rc<MemoryMap>>, SimError> {
    let mut memory_maps = HashMap::new();
    for memory_map in &cfg.memory_maps {
        let built = build_memory_map(memory_map, memories, memories_idx_by_id, device_ids)?;
        memory_maps.insert(memory_map.name.clone(), Rc::new(built));
    }

    Ok(memory_maps)
}

pub const DEFAULT_PE_NUM_ACTIVE_REQUESTS: usize = 8;
pub const DEFAULT_PE_LSU_ACCESS_BYTES: usize = 32;
pub const DEFAULT_PE_SRAM_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_PE_ADDS_PER_TICK: f64 = 16.0;
pub const DEFAULT_PE_MULS_PER_TICK: f64 = 4.0;
pub const DEFAULT_PE_COMPARES_PER_TICK: f64 = DEFAULT_PE_ADDS_PER_TICK;
pub const DEFAULT_PE_OVERHEAD_SIZE_BYTES: usize = 8;

fn build_pe_config(
    cfg: &ProcessingElementConfigSection,
) -> Result<ProcessingElementConfig, SimError> {
    let num_active_requests = cfg
        .num_active_requests
        .unwrap_or(DEFAULT_PE_NUM_ACTIVE_REQUESTS);
    let lsu_access_bytes = cfg.lsu_access_bytes.unwrap_or(DEFAULT_PE_LSU_ACCESS_BYTES);
    let overhead_size_bytes = cfg
        .overhead_size_bytes
        .unwrap_or(DEFAULT_PE_OVERHEAD_SIZE_BYTES);
    let sram_bytes = cfg.sram_bytes.unwrap_or(DEFAULT_PE_SRAM_BYTES) as usize;

    let adds_per_tick = cfg.adds_per_tick.unwrap_or(DEFAULT_PE_ADDS_PER_TICK);
    let muls_per_tick = cfg.muls_per_tick.unwrap_or(DEFAULT_PE_MULS_PER_TICK);
    let compares_per_tick = cfg
        .compares_per_tick
        .unwrap_or(DEFAULT_PE_COMPARES_PER_TICK);

    Ok(ProcessingElementConfig {
        num_active_requests,
        lsu_access_bytes,
        overhead_size_bytes,
        sram_bytes,
        adds_per_tick,
        muls_per_tick,
        compares_per_tick,
    })
}

pub fn build_pes<S: BuildHasher>(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
    memory_maps: &HashMap<String, Rc<MemoryMap>, S>,
    device_ids: &DeviceIds,
) -> Result<(ProcessingElements, NameToIdxMap), SimError> {
    let mut processing_elements = Vec::new();
    if let Some(pes) = &cfg.processing_elements {
        for pe_section in pes {
            let memory_map = memory_maps
                .get(pe_section.memory_map.as_str())
                .ok_or_else(|| {
                    SimError(format!("Unknown memory map '{}'", pe_section.memory_map))
                })?;
            let device_id = *device_ids
                .get(&pe_section.name)
                .ok_or_else(|| SimError(format!("Unknown device '{}'", pe_section.name)))?;
            let pe_config = build_pe_config(&pe_section.config)?;
            processing_elements.push(ProcessingElement::new_and_register(
                engine,
                clock,
                parent,
                pe_section.name.as_str(),
                memory_map,
                &pe_config,
                device_id,
            )?);
        }
    }
    let mut pes_idx_by_id = HashMap::new();
    for (i, pe) in processing_elements.iter().enumerate() {
        let name = pe.entity().name.to_string();
        pes_idx_by_id.insert(name, i);
    }
    Ok((processing_elements, pes_idx_by_id))
}

pub const DEFAULT_CACHE_LINE_SIZE_BYTES: usize = 32;
pub const DEFAULT_CACHE_BW_BYTES_PER_CYCLE: usize = 32;
pub const DEFAULT_CACHE_NUM_WAYS: usize = 4;
pub const DEFAULT_CACHE_NUM_SETS: usize = 128;
pub const DEFAULT_CACHE_LATENCY_TICKS: usize = 20;

pub fn build_caches(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<(Caches, NameToIdxMap), SimError> {
    let mut caches = Vec::new();
    if let Some(caches_sections) = &cfg.caches {
        for cache_section in caches_sections {
            let bw_bytes_per_cycle = cache_section
                .config
                .bw_bytes_per_cycle
                .unwrap_or(DEFAULT_CACHE_BW_BYTES_PER_CYCLE);
            let line_size_bytes = cache_section
                .config
                .line_size_bytes
                .unwrap_or(DEFAULT_CACHE_LINE_SIZE_BYTES);
            let num_sets = cache_section
                .config
                .num_sets
                .unwrap_or(DEFAULT_CACHE_NUM_SETS);
            let num_ways = cache_section
                .config
                .num_ways
                .unwrap_or(DEFAULT_CACHE_NUM_WAYS);
            let delay_ticks = cache_section
                .config
                .delay_ticks
                .unwrap_or(DEFAULT_CACHE_LATENCY_TICKS);

            let config = CacheConfig::new(
                line_size_bytes,
                bw_bytes_per_cycle,
                num_sets,
                num_ways,
                delay_ticks,
            );
            caches.push(Cache::new_and_register(
                engine,
                clock,
                parent,
                cache_section.name.as_str(),
                config,
            )?);
        }
    }

    let mut caches_idx_by_id = HashMap::new();
    for (i, pe) in caches.iter().enumerate() {
        let name = pe.entity().name.to_string();
        caches_idx_by_id.insert(name, i);
    }

    Ok((caches, caches_idx_by_id))
}

pub const DEFAULT_FABRIC_PORTS_PER_NODE: usize = 1;
pub const DEFAULT_FABRIC_TICKS_PER_HOP: usize = 2;
pub const DEFAULT_FABRIC_TICKS_OVERHEAD: usize = 10;
pub const DEFAULT_FABRIC_RX_BUFFER_BYTES: usize = 256;
pub const DEFAULT_FABRIC_TX_BUFFER_BYTES: usize = 256;
pub const DEFAULT_FABRIC_PORT_BITS_PER_TICK: usize = 32 * 8; // 32 bytes per cycle
pub const DEFAULT_FABRIC_ROUTING: FabricRoutingAlgorithm = FabricRoutingAlgorithm::ColumnFirst;

pub fn build_fabrics(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<(Fabrics, NameToIdxMap), SimError> {
    let mut fabrics = Vec::new();
    if let Some(fabric_sections) = &cfg.fabrics {
        for fabric_section in fabric_sections {
            let fabric_columns = fabric_section.columns;
            let fabric_rows = fabric_section.rows;
            let fabric_ports_per_node = fabric_section
                .fabric_ports_per_node
                .unwrap_or(DEFAULT_FABRIC_PORTS_PER_NODE);
            let ticks_per_hop = fabric_section
                .ticks_per_hop
                .unwrap_or(DEFAULT_FABRIC_TICKS_PER_HOP);
            let ticks_overhead = fabric_section
                .ticks_overhead
                .unwrap_or(DEFAULT_FABRIC_TICKS_OVERHEAD);
            let rx_buffer_bytes = fabric_section
                .rx_buffer_bytes
                .unwrap_or(DEFAULT_FABRIC_RX_BUFFER_BYTES);
            let tx_buffer_bytes = fabric_section
                .tx_buffer_bytes
                .unwrap_or(DEFAULT_FABRIC_TX_BUFFER_BYTES);
            let port_bits_per_tick = fabric_section
                .port_bits_per_tick
                .unwrap_or(DEFAULT_FABRIC_PORT_BITS_PER_TICK);
            let fabric_algorithm = fabric_section.routing.unwrap_or(DEFAULT_FABRIC_ROUTING);

            let config = Rc::new(FabricConfig::new(
                fabric_columns,
                fabric_rows,
                fabric_ports_per_node,
                None,
                ticks_per_hop,
                ticks_overhead,
                rx_buffer_bytes,
                tx_buffer_bytes,
                port_bits_per_tick,
            ));

            let fabric: Rc<dyn Fabric<MemoryAccess>> = match fabric_section.kind {
                FabricKind::Functional => FunctionalFabric::new_and_register(
                    engine,
                    clock,
                    parent,
                    &fabric_section.name,
                    config.clone(),
                )?,
                FabricKind::Routed => RoutedFabric::new_and_register(
                    engine,
                    clock,
                    parent,
                    &fabric_section.name,
                    config.clone(),
                    fabric_algorithm,
                )?,
            };
            fabrics.push(fabric);
        }
    }

    let mut fabrics_idx_by_id = HashMap::new();
    for (i, fabric) in fabrics.iter().enumerate() {
        let name = fabric.entity().name.to_string();
        fabrics_idx_by_id.insert(name, i);
    }

    Ok((fabrics, fabrics_idx_by_id))
}

pub const DEFAULT_HBM_DELAY_TICKS: usize = 10;
pub const DEFAULT_HBM_BW_BYTES_PER_CYCLE: usize = 32;
pub const DEFAULT_HBM_SIZE_BYTES: usize = 1024 * 1024 * 1024;

pub fn build_memories(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<(Memories, NameToIdxMap), SimError> {
    let mut memories = Vec::new();
    if let Some(memories_section) = &cfg.memories {
        for memory_section in memories_section {
            let base_address = memory_section.base_address;
            let capacity_bytes = memory_section.capacity_bytes as usize;
            let bw_bytes_per_cycle = memory_section
                .bw_bytes_per_cycle
                .unwrap_or(DEFAULT_HBM_BW_BYTES_PER_CYCLE);
            let delay_ticks = memory_section
                .delay_ticks
                .unwrap_or(DEFAULT_HBM_DELAY_TICKS);
            let config = MemoryConfig::new(
                base_address,
                capacity_bytes,
                bw_bytes_per_cycle,
                delay_ticks,
            );
            memories.push(Memory::new_and_register(
                engine,
                clock,
                parent,
                memory_section.name.as_str(),
                config,
            )?);
        }
    }

    let mut memories_idx_by_id = HashMap::new();
    for (i, memory) in memories.iter().enumerate() {
        let name = memory.entity().name.to_string();
        memories_idx_by_id.insert(name, i);
    }

    Ok((memories, memories_idx_by_id))
}

#[cfg(test)]
mod tests {
    use gwr_engine::test_helpers::start_test;
    use gwr_models::memory::memory_map::DeviceId;

    use super::{build_memories, build_memory_maps};
    use crate::DeviceIds;
    use crate::types::{
        MemoryDeviceSection, MemoryKind, MemoryMapSection, MemorySection, PlatformConfig,
    };

    #[test]
    fn builds_runtime_memory_maps_from_built_memories() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let cfg = PlatformConfig {
            memory_maps: vec![MemoryMapSection {
                name: "mm0".to_string(),
                devices: vec![MemoryDeviceSection {
                    name: "hbm0".to_string(),
                }],
            }],
            processing_elements: None,
            caches: None,
            fabrics: None,
            memories: Some(vec![MemorySection {
                name: "hbm0".to_string(),
                kind: MemoryKind::HBM,
                base_address: 0x4000,
                capacity_bytes: 0x2000,
                bw_bytes_per_cycle: None,
                delay_ticks: None,
            }]),
            connections: None,
        };
        let device_ids = DeviceIds::from([("hbm0".to_string(), DeviceId(7))]);
        let (memories, memories_idx_by_id) = build_memories(&engine, &clock, engine.top(), &cfg)
            .expect("memory build should succeed");

        let memory_maps = build_memory_maps(&cfg, &memories, &memories_idx_by_id, &device_ids)
            .expect("memory maps should build");
        let memory_map = memory_maps.get("mm0").expect("memory map should exist");

        assert_eq!(memory_map.num_regions(), 1);
        assert_eq!(memory_map.lookup(0x4000), Some((DeviceId(7), 0)));
        assert_eq!(memory_map.lookup(0x5fff), Some((DeviceId(7), 0x1fff)));
        assert_eq!(memory_map.lookup(0x6000), None);
    }
}
