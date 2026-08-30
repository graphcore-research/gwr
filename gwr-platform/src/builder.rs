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
use gwr_models::fabric::{Fabric, FabricConfig, FabricGeometry, FabricPortConfig};
use gwr_models::memory::cache::{Cache, CacheConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::processing_element::{ProcessingElement, ProcessingElementConfig};
use gwr_track::entity::{Entity, GetEntity};

use crate::types::{
    CacheConfigSection, CacheSection, FabricConfigSection, FabricKind, FabricSection,
    MemoryConfigSection, MemoryMapSection, MemorySection, PlatformConfig,
    ProcessingElementConfigSection, ProcessingElementSection,
};
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

pub(crate) struct EffectiveConfigs {
    pub(crate) processing_elements: Vec<ProcessingElementConfig>,
    pub(crate) caches: Vec<CacheConfig>,
    pub(crate) fabrics: Vec<(Rc<FabricConfig>, FabricRoutingAlgorithm)>,
    pub(crate) memories: Vec<MemoryConfig>,
}

impl EffectiveConfigs {
    pub(crate) fn new(platform: &PlatformConfig) -> Result<Self, SimError> {
        Ok(Self {
            processing_elements: processing_element_configs(platform)?,
            caches: cache_configs(platform)?,
            fabrics: fabric_configs(platform)?,
            memories: memory_configs(platform)?,
        })
    }
}

pub const DEFAULT_PE_NUM_ACTIVE_REQUESTS: usize = 8;
pub const DEFAULT_PE_LSU_ACCESS_BYTES: usize = 32;
pub const DEFAULT_PE_SRAM_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_PE_ADDS_PER_TICK: f64 = 16.0;
pub const DEFAULT_PE_MULS_PER_TICK: f64 = 4.0;
pub const DEFAULT_PE_COMPARES_PER_TICK: f64 = DEFAULT_PE_ADDS_PER_TICK;
pub const DEFAULT_PE_OVERHEAD_SIZE_BYTES: usize = 8;

impl ProcessingElementConfigSection {
    pub(crate) fn model_config(&self) -> Result<ProcessingElementConfig, SimError> {
        let num_active_requests = self
            .num_active_requests
            .unwrap_or(DEFAULT_PE_NUM_ACTIVE_REQUESTS);
        let sram_bytes = usize::try_from(self.sram_bytes.unwrap_or(DEFAULT_PE_SRAM_BYTES))
            .map_err(|error| {
                SimError(format!("SRAM size cannot be represented as usize: {error}"))
            })?;
        let config = ProcessingElementConfig {
            num_active_requests,
            lsu_access_bytes: self.lsu_access_bytes.unwrap_or(DEFAULT_PE_LSU_ACCESS_BYTES),
            overhead_size_bytes: self
                .overhead_size_bytes
                .unwrap_or(DEFAULT_PE_OVERHEAD_SIZE_BYTES),
            sram_bytes,
            adds_per_tick: self.adds_per_tick.unwrap_or(DEFAULT_PE_ADDS_PER_TICK),
            muls_per_tick: self.muls_per_tick.unwrap_or(DEFAULT_PE_MULS_PER_TICK),
            compares_per_tick: self
                .compares_per_tick
                .unwrap_or(DEFAULT_PE_COMPARES_PER_TICK),
        };
        config.validate()?;
        Ok(config)
    }
}

impl ProcessingElementSection {
    /// Return the model configuration after applying platform defaults.
    pub fn effective_config(&self) -> Result<ProcessingElementConfig, SimError> {
        self.config
            .model_config()
            .map_err(|error| SimError(format!("Processing element '{}': {error}", self.name)))
    }
}

fn processing_element_configs(
    platform: &PlatformConfig,
) -> Result<Vec<ProcessingElementConfig>, SimError> {
    platform
        .processing_elements
        .iter()
        .flatten()
        .map(ProcessingElementSection::effective_config)
        .collect()
}

fn cache_configs(platform: &PlatformConfig) -> Result<Vec<CacheConfig>, SimError> {
    platform
        .caches
        .iter()
        .flatten()
        .map(CacheSection::effective_config)
        .collect()
}

fn fabric_configs(
    platform: &PlatformConfig,
) -> Result<Vec<(Rc<FabricConfig>, FabricRoutingAlgorithm)>, SimError> {
    platform
        .fabrics
        .iter()
        .flatten()
        .map(FabricSection::effective_config)
        .map(|result| result.map(|(config, routing)| (Rc::new(config), routing)))
        .collect()
}

fn memory_configs(platform: &PlatformConfig) -> Result<Vec<MemoryConfig>, SimError> {
    platform
        .memories
        .iter()
        .flatten()
        .map(MemorySection::effective_config)
        .collect()
}

pub fn build_pes<S: BuildHasher>(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
    memory_maps: &HashMap<String, Rc<MemoryMap>, S>,
    device_ids: &DeviceIds,
) -> Result<(ProcessingElements, NameToIdxMap), SimError> {
    let configs = processing_element_configs(cfg)?;
    build_pes_from_configs(
        engine,
        clock,
        parent,
        cfg,
        &configs,
        memory_maps,
        device_ids,
    )
}

pub(crate) fn build_pes_from_configs<S: BuildHasher>(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
    configs: &[ProcessingElementConfig],
    memory_maps: &HashMap<String, Rc<MemoryMap>, S>,
    device_ids: &DeviceIds,
) -> Result<(ProcessingElements, NameToIdxMap), SimError> {
    let mut processing_elements = Vec::new();
    if let Some(pes) = &cfg.processing_elements {
        for (pe_section, pe_config) in pes.iter().zip(configs) {
            let memory_map = memory_maps
                .get(pe_section.memory_map.as_str())
                .ok_or_else(|| {
                    SimError(format!("Unknown memory map '{}'", pe_section.memory_map))
                })?;
            let device_id = *device_ids
                .get(&pe_section.name)
                .ok_or_else(|| SimError(format!("Unknown device '{}'", pe_section.name)))?;
            processing_elements.push(ProcessingElement::new_and_register(
                engine,
                clock,
                parent,
                pe_section.name.as_str(),
                memory_map,
                pe_config,
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
pub const DEFAULT_CACHE_BW_BYTES_PER_TICK: usize = 32;
pub const DEFAULT_CACHE_NUM_WAYS: usize = 4;
pub const DEFAULT_CACHE_NUM_SETS: usize = 128;
pub const DEFAULT_CACHE_LATENCY_TICKS: usize = 20;

impl CacheConfigSection {
    pub(crate) fn model_config(&self) -> Result<CacheConfig, SimError> {
        let config = CacheConfig::new(
            self.line_size_bytes
                .unwrap_or(DEFAULT_CACHE_LINE_SIZE_BYTES),
            self.bw_bytes_per_tick
                .unwrap_or(DEFAULT_CACHE_BW_BYTES_PER_TICK),
            self.num_sets.unwrap_or(DEFAULT_CACHE_NUM_SETS),
            self.num_ways.unwrap_or(DEFAULT_CACHE_NUM_WAYS),
            self.delay_ticks.unwrap_or(DEFAULT_CACHE_LATENCY_TICKS),
        );
        config.validate()?;
        Ok(config)
    }
}

impl CacheSection {
    fn effective_config(&self) -> Result<CacheConfig, SimError> {
        self.config
            .model_config()
            .map_err(|error| SimError(format!("Cache '{}': {error}", self.name)))
    }
}

pub fn build_caches(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<(Caches, NameToIdxMap), SimError> {
    let configs = cache_configs(cfg)?;
    build_caches_from_configs(engine, clock, parent, cfg, &configs)
}

pub(crate) fn build_caches_from_configs(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
    configs: &[CacheConfig],
) -> Result<(Caches, NameToIdxMap), SimError> {
    let mut caches = Vec::new();
    if let Some(caches_sections) = &cfg.caches {
        for (cache_section, config) in caches_sections.iter().zip(configs) {
            caches.push(Cache::new_and_register(
                engine,
                clock,
                parent,
                cache_section.name.as_str(),
                config.clone(),
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
pub const DEFAULT_FABRIC_PORT_BITS_PER_TICK: usize = 32 * 8; // 32 bytes per tick
pub const DEFAULT_FABRIC_ROUTING: FabricRoutingAlgorithm = FabricRoutingAlgorithm::ColumnFirst;

impl FabricConfigSection {
    pub(crate) fn model_config(
        &self,
        num_columns: usize,
        num_rows: usize,
    ) -> Result<(FabricConfig, FabricRoutingAlgorithm), SimError> {
        let config = FabricConfig::new(
            FabricGeometry {
                num_columns,
                num_rows,
                num_ports_per_node: self
                    .fabric_ports_per_node
                    .unwrap_or(DEFAULT_FABRIC_PORTS_PER_NODE),
                ports_per_node_limit: None,
            },
            FabricPortConfig {
                ticks_per_hop: self.ticks_per_hop.unwrap_or(DEFAULT_FABRIC_TICKS_PER_HOP),
                ticks_overhead: self.ticks_overhead.unwrap_or(DEFAULT_FABRIC_TICKS_OVERHEAD),
                rx_buffer_bytes: self
                    .rx_buffer_bytes
                    .unwrap_or(DEFAULT_FABRIC_RX_BUFFER_BYTES),
                tx_buffer_bytes: self
                    .tx_buffer_bytes
                    .unwrap_or(DEFAULT_FABRIC_TX_BUFFER_BYTES),
                port_bits_per_tick: self
                    .port_bits_per_tick
                    .unwrap_or(DEFAULT_FABRIC_PORT_BITS_PER_TICK),
            },
        )?;
        Ok((config, self.routing.unwrap_or(DEFAULT_FABRIC_ROUTING)))
    }
}

impl FabricSection {
    fn effective_config(&self) -> Result<(FabricConfig, FabricRoutingAlgorithm), SimError> {
        self.config
            .model_config(self.columns, self.rows)
            .map_err(|error| SimError(format!("Fabric '{}': {error}", self.name)))
    }
}

pub fn build_fabrics(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<(Fabrics, NameToIdxMap), SimError> {
    let configs = fabric_configs(cfg)?;
    build_fabrics_from_configs(engine, clock, parent, cfg, &configs)
}

pub(crate) fn build_fabrics_from_configs(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
    configs: &[(Rc<FabricConfig>, FabricRoutingAlgorithm)],
) -> Result<(Fabrics, NameToIdxMap), SimError> {
    let mut fabrics = Vec::new();
    if let Some(fabric_sections) = &cfg.fabrics {
        for (fabric_section, (config, fabric_algorithm)) in fabric_sections.iter().zip(configs) {
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
                    *fabric_algorithm,
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
pub const DEFAULT_HBM_BW_BYTES_PER_TICK: usize = 32;
pub const DEFAULT_HBM_SIZE_BYTES: usize = 1024 * 1024 * 1024;

impl MemoryConfigSection {
    pub(crate) fn model_config(&self, base_address: u64) -> Result<MemoryConfig, SimError> {
        let capacity_bytes = usize::try_from(self.capacity_bytes).map_err(|error| {
            SimError(format!("capacity cannot be represented as usize: {error}"))
        })?;
        let config = MemoryConfig::new(
            base_address,
            capacity_bytes,
            self.bw_bytes_per_tick
                .unwrap_or(DEFAULT_HBM_BW_BYTES_PER_TICK),
            self.delay_ticks.unwrap_or(DEFAULT_HBM_DELAY_TICKS),
        );
        config.validate()?;
        Ok(config)
    }
}

impl MemorySection {
    fn effective_config(&self) -> Result<MemoryConfig, SimError> {
        self.config
            .model_config(self.base_address)
            .map_err(|error| SimError(format!("Memory '{}': {error}", self.name)))
    }
}

pub fn build_memories(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
) -> Result<(Memories, NameToIdxMap), SimError> {
    let configs = memory_configs(cfg)?;
    build_memories_from_configs(engine, clock, parent, cfg, &configs)
}

pub(crate) fn build_memories_from_configs(
    engine: &Engine,
    clock: &Clock,
    parent: &Rc<Entity>,
    cfg: &PlatformConfig,
    configs: &[MemoryConfig],
) -> Result<(Memories, NameToIdxMap), SimError> {
    let mut memories = Vec::new();
    if let Some(memories_section) = &cfg.memories {
        for (memory_section, config) in memories_section.iter().zip(configs) {
            memories.push(Memory::new_and_register(
                engine,
                clock,
                parent,
                memory_section.name.as_str(),
                config.clone(),
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
        MemoryConfigSection, MemoryDeviceSection, MemoryKind, MemoryMapSection, MemorySection,
        PlatformConfig,
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
            defaults: None,
            processing_elements: None,
            caches: None,
            fabrics: None,
            memories: Some(vec![MemorySection {
                name: "hbm0".to_string(),
                kind: MemoryKind::HBM,
                base_address: 0x4000,
                config: MemoryConfigSection {
                    capacity_bytes: 0x2000,
                    bw_bytes_per_tick: None,
                    delay_ticks: None,
                },
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
