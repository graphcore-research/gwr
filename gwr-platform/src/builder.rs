// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

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
use gwr_track::entity::Entity;

use crate::types::{FabricKind, MemoryMapSection, PlatformConfig, ProcessingElementConfigSection};
use crate::{Caches, DeviceIds, Fabrics, Memories, ProcessingElements};

pub fn build_memory_map(
    cfg: &MemoryMapSection,
    device_ids: &DeviceIds,
) -> Result<MemoryMap, SimError> {
    let mut memory_map = MemoryMap::new();
    for range in &cfg.ranges {
        let start = range.base_address;
        let size = range.size_bytes;
        let device_id = *device_ids
            .get(&range.device)
            .ok_or_else(|| SimError(format!("Unknown device '{}'", range.device)))?;
        memory_map.insert(start, size, device_id)?;
    }
    Ok(memory_map)
}

fn build_pe_config(
    cfg: &ProcessingElementConfigSection,
) -> Result<ProcessingElementConfig, SimError> {
    const DEFAULT_NUM_ACTIVE_REQUESTS: usize = 8;
    const DEFAULT_LSU_ACCESS_BYTES: usize = 32;
    const DEFAULT_OVERHEAD_SIZE_BYTES: usize = 8;
    // Assume a default of 1MB of local SRAM
    const DEFAULT_SRAM_BYTES: u64 = 1024 * 1024;
    const DEFAULT_ADDS_PER_TICK: usize = 16;
    const DEFAULT_MULS_PER_TICK: usize = 4;

    let num_active_requests = cfg
        .num_active_requests
        .unwrap_or(DEFAULT_NUM_ACTIVE_REQUESTS);
    let lsu_access_bytes = cfg.lsu_access_bytes.unwrap_or(DEFAULT_LSU_ACCESS_BYTES);
    let overhead_size_bytes = cfg
        .overhead_size_bytes
        .unwrap_or(DEFAULT_OVERHEAD_SIZE_BYTES);
    let sram_bytes = cfg.sram_bytes.unwrap_or(DEFAULT_SRAM_BYTES) as usize;

    let adds_per_tick = cfg.adds_per_tick.unwrap_or(DEFAULT_ADDS_PER_TICK);
    let muls_per_tick = cfg.muls_per_tick.unwrap_or(DEFAULT_MULS_PER_TICK);

    Ok(ProcessingElementConfig {
        num_active_requests,
        lsu_access_bytes,
        overhead_size_bytes,
        sram_bytes,
        adds_per_tick,
        muls_per_tick,
    })
}

pub fn build_pes(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
    device_ids: &DeviceIds,
) -> Result<ProcessingElements, SimError> {
    let mut processing_elements = Vec::new();
    if let Some(pes) = &cfg.processing_elements {
        for pe_section in pes {
            let memory_map = Rc::new(build_memory_map(&pe_section.memory_map, device_ids)?);
            let device_id = *device_ids
                .get(&pe_section.name)
                .ok_or_else(|| SimError(format!("Unknown device '{}'", pe_section.name)))?;
            let pe_config = build_pe_config(&pe_section.config)?;
            processing_elements.push(ProcessingElement::new_and_register(
                engine,
                clock,
                parent,
                pe_section.name.as_str(),
                &memory_map,
                &pe_config,
                device_id,
            )?);
        }
    }
    Ok(processing_elements)
}

pub fn build_caches(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<Caches, SimError> {
    const DEFAULT_BW_BYTES_PER_CYCLE: usize = 32;
    const DEFAULT_LINE_SIZE_BYTES: usize = 32;
    const DEFAULT_NUM_WAYS: usize = 4;
    const DEFAULT_NUM_SETS: usize = 128;
    const DEFAULT_LATENCY_TICKS: usize = 20;

    let mut caches = Vec::new();
    if let Some(caches_sections) = &cfg.caches {
        for cache_section in caches_sections {
            let bw_bytes_per_cycle = cache_section
                .bw_bytes_per_cycle
                .unwrap_or(DEFAULT_BW_BYTES_PER_CYCLE);
            let line_size_bytes = cache_section
                .line_size_bytes
                .unwrap_or(DEFAULT_LINE_SIZE_BYTES);
            let num_sets = cache_section.num_sets.unwrap_or(DEFAULT_NUM_SETS);
            let num_ways = cache_section.num_ways.unwrap_or(DEFAULT_NUM_WAYS);
            let delay_ticks = cache_section.delay_ticks.unwrap_or(DEFAULT_LATENCY_TICKS);

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
    Ok(caches)
}

pub fn build_fabrics(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<Fabrics, SimError> {
    const DEFAULT_FABRIC_PORTS_PER_NODE: usize = 1;
    const DEFAULT_TICKS_PER_HOP: usize = 2;
    const DEFAULT_TICKS_OVERHEAD: usize = 10;
    const DEFAULT_RX_BUFFER_ENTRIES: usize = 256;
    const DEFAULT_TX_BUFFER_ENTRIES: usize = 256;
    const DEFAULT_PORT_BITS_PER_TICK: usize = 32 * 8; // 32 bytes per cycle
    const DEFAULT_ROUTING: FabricRoutingAlgorithm = FabricRoutingAlgorithm::ColumnFirst;

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
                .unwrap_or(DEFAULT_TICKS_PER_HOP);
            let ticks_overhead = fabric_section
                .ticks_overhead
                .unwrap_or(DEFAULT_TICKS_OVERHEAD);
            let rx_buffer_entries = fabric_section
                .rx_buffer_entries
                .unwrap_or(DEFAULT_RX_BUFFER_ENTRIES);
            let tx_buffer_entries = fabric_section
                .tx_buffer_entries
                .unwrap_or(DEFAULT_TX_BUFFER_ENTRIES);
            let port_bits_per_tick = fabric_section
                .port_bits_per_tick
                .unwrap_or(DEFAULT_PORT_BITS_PER_TICK);
            let fabric_algorithm = fabric_section.routing.unwrap_or(DEFAULT_ROUTING);

            let config = Rc::new(FabricConfig::new(
                fabric_columns,
                fabric_rows,
                fabric_ports_per_node,
                None,
                ticks_per_hop,
                ticks_overhead,
                rx_buffer_entries,
                tx_buffer_entries,
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
    Ok(fabrics)
}

pub fn build_memories(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<Memories, SimError> {
    const DEFAULT_BW_BYTES_PER_CYCLE: usize = 32;
    const DEFAULT_DELAY_TICKS: usize = 10;

    let mut memories = Vec::new();
    if let Some(memories_section) = &cfg.memories {
        for memory_section in memories_section {
            let base_address = memory_section.base_address;
            let capacity_bytes = memory_section.capacity_bytes as usize;
            let bw_bytes_per_cycle = memory_section
                .bw_bytes_per_cycle
                .unwrap_or(DEFAULT_BW_BYTES_PER_CYCLE);
            let delay_ticks = memory_section.delay_ticks.unwrap_or(DEFAULT_DELAY_TICKS);
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
    Ok(memories)
}
