// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::Path;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimError;
use gwr_model_builder::EntityGet;
use gwr_models::fabric::Fabric;
use gwr_models::memory::Memory;
use gwr_models::memory::cache::Cache;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::DeviceId;
use gwr_models::processing_element::ProcessingElement;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_track::entity::{Entity, GetEntity};

use crate::builder::{build_caches, build_fabrics, build_memories, build_pes};
use crate::connect::connect_ports;
use crate::types::PlatformConfig;

pub mod builder;
mod connect;
pub mod types;

type ProcessingElements = Vec<Rc<ProcessingElement>>;
type Caches = Vec<Rc<Cache<MemoryAccess>>>;
type Fabrics = Vec<Rc<dyn Fabric<MemoryAccess>>>;
type Memories = Vec<Rc<Memory<MemoryAccess>>>;
type DeviceIds = HashMap<String, DeviceId>;

#[derive(EntityGet)]
pub struct Platform {
    entity: Rc<Entity>,
    processing_elements: ProcessingElements,
    pes_idx_by_id: HashMap<String, usize>,
    caches: Caches,
    caches_idx_by_id: HashMap<String, usize>,
    fabrics: Fabrics,
    fabrics_idx_by_id: HashMap<String, usize>,
    memories: Memories,
    memories_idx_by_id: HashMap<String, usize>,
}

impl Platform {
    pub fn from_file(
        engine: &Engine,
        clock: &Clock,
        platform_path: &Path,
    ) -> Result<Self, SimError> {
        let s = std::fs::read_to_string(platform_path)
            .map_err(|e| SimError(format!("Unable to read {}: {e}", platform_path.display())))?;
        Platform::from_string(engine, clock, &s)
    }

    pub fn from_string(
        engine: &Engine,
        clock: &Clock,
        platform_config: &str,
    ) -> Result<Self, SimError> {
        let cfg: PlatformConfig = serde_yaml::from_str(platform_config)
            .map_err(|e| SimError(format!("serde_yaml::from_str failed: {e}")))?;
        Platform::build(engine, clock, &cfg)
    }

    fn build(engine: &Engine, clock: &Clock, cfg: &PlatformConfig) -> Result<Self, SimError> {
        let device_ids = assign_device_ids(cfg)?;

        let top = engine.top();

        let processing_elements = build_pes(engine, clock, top, cfg, &device_ids)?;
        let mut pes_idx_by_id = HashMap::new();
        for (i, pe) in processing_elements.iter().enumerate() {
            let name = pe.entity().name.to_string();
            pes_idx_by_id.insert(name, i);
        }
        let caches = build_caches(engine, clock, top, cfg)?;
        let mut caches_idx_by_id = HashMap::new();
        for (i, pe) in caches.iter().enumerate() {
            let name = pe.entity().name.to_string();
            caches_idx_by_id.insert(name, i);
        }

        let fabrics = build_fabrics(engine, clock, top, cfg)?;
        let mut fabrics_idx_by_id = HashMap::new();
        for (i, fabric) in fabrics.iter().enumerate() {
            let name = fabric.entity().name.to_string();
            fabrics_idx_by_id.insert(name, i);
        }

        let memories = build_memories(engine, clock, top, cfg)?;
        let mut memories_idx_by_id = HashMap::new();
        for (i, memory) in memories.iter().enumerate() {
            let name = memory.entity().name.to_string();
            memories_idx_by_id.insert(name, i);
        }

        let parent = engine.top();
        let entity = Rc::new(Entity::new(parent, "platform"));
        let platform = Platform {
            entity,
            processing_elements,
            pes_idx_by_id,
            caches,
            caches_idx_by_id,
            fabrics,
            fabrics_idx_by_id,
            memories,
            memories_idx_by_id,
        };
        connect_ports(&platform, cfg)?;
        Ok(platform)
    }

    pub fn cache_idx_from_name(&self, cache_name: &str) -> Result<usize, SimError> {
        match self.caches_idx_by_id.get(cache_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No Cache '{cache_name}'"),
        }
    }

    pub fn fabric_idx_from_name(&self, fabric_name: &str) -> Result<usize, SimError> {
        match self.fabrics_idx_by_id.get(fabric_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No Fabric '{fabric_name}'"),
        }
    }

    pub fn memory_idx_from_name(&self, memory_name: &str) -> Result<usize, SimError> {
        match self.memories_idx_by_id.get(memory_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No Memory '{memory_name}'"),
        }
    }

    pub fn pe_idx_from_name(&self, pe_name: &str) -> Result<usize, SimError> {
        match self.pes_idx_by_id.get(pe_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No PE '{pe_name}'"),
        }
    }

    #[must_use]
    pub fn num_caches(&self) -> usize {
        self.caches_idx_by_id.keys().len()
    }

    #[must_use]
    pub fn num_fabrics(&self) -> usize {
        self.fabrics_idx_by_id.keys().len()
    }

    #[must_use]
    pub fn num_memories(&self) -> usize {
        self.memories_idx_by_id.keys().len()
    }

    #[must_use]
    pub fn num_pes(&self) -> usize {
        self.pes_idx_by_id.keys().len()
    }

    #[must_use]
    pub fn pe_names(&self) -> Vec<String> {
        self.pes_idx_by_id
            .keys()
            .map(|pe_name| pe_name.to_string())
            .collect()
    }

    pub fn cache(&self, cache_name: &str) -> Result<&Rc<Cache<MemoryAccess>>, SimError> {
        let idx = self.cache_idx_from_name(cache_name)?;
        Ok(&self.caches[idx])
    }

    pub fn fabric(&self, fabric_name: &str) -> Result<&Rc<dyn Fabric<MemoryAccess>>, SimError> {
        let idx = self.fabric_idx_from_name(fabric_name)?;
        Ok(&self.fabrics[idx])
    }

    pub fn memory(&self, memory_name: &str) -> Result<&Rc<Memory<MemoryAccess>>, SimError> {
        let idx = self.memory_idx_from_name(memory_name)?;
        Ok(&self.memories[idx])
    }

    pub fn pe(&self, pe_name: &str) -> Result<&Rc<ProcessingElement>, SimError> {
        let idx = self.pe_idx_from_name(pe_name)?;
        Ok(&self.processing_elements[idx])
    }

    pub fn attach_dispatcher(&self, dispatcher: &Rc<dyn Dispatch>) {
        for pe in &self.processing_elements {
            pe.set_dispatcher(dispatcher);
        }
    }
}

impl Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "ProcessingElements:")?;
        for (i, pe) in self.processing_elements.iter().enumerate() {
            writeln!(f, "  {i}: {}", pe.entity())?;
        }

        writeln!(f, "\nMemories:")?;
        for (i, mem) in self.memories.iter().enumerate() {
            writeln!(f, "  {i}: {}", mem.entity())?;
        }

        writeln!(f, "\nCaches:")?;
        for (i, cache) in self.caches.iter().enumerate() {
            writeln!(f, "  {i}: {}", cache.entity())?;
        }

        writeln!(f, "\nFabrics:")?;
        for (i, fabric) in self.fabrics.iter().enumerate() {
            writeln!(f, "  {i}: {}", fabric.entity())?;
        }

        Ok(())
    }
}

fn assign_device_ids(cfg: &PlatformConfig) -> Result<DeviceIds, SimError> {
    let mut device_id = 0;
    let mut device_ids = DeviceIds::new();
    if let Some(pes) = &cfg.processing_elements {
        for pe in pes {
            if device_ids
                .insert(pe.name.to_string(), DeviceId(device_id))
                .is_some()
            {
                return sim_error!("Duplicate device name {}", pe.name);
            }
            device_id += 1;
        }
    }
    if let Some(mems) = &cfg.memories {
        for mem in mems {
            if device_ids
                .insert(mem.name.to_string(), DeviceId(device_id))
                .is_some()
            {
                return sim_error!("Duplicate device name {}", mem.name);
            }
            device_id += 1;
        }
    }
    Ok(device_ids)
}
