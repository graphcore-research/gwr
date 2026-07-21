// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![doc = include_str!(gwr_build::generated_crate_docs_path!())]

use std::collections::HashMap;
use std::fmt::{self, Display};
use std::path::Path;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimError;
use gwr_model_builder::EntityGet;
use gwr_models::fabric::Fabric;
use gwr_models::log_stats;
use gwr_models::memory::cache::{Cache, CacheStatsDisplay};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::DeviceId;
use gwr_models::memory::{Memory, MemoryStatsDisplay};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::{
    MachineOpCounts, ProcessingElement, ProcessingElementStatsDisplay,
};
use gwr_track::entity::{Entity, GetEntity};

use crate::builder::{build_caches, build_fabrics, build_memories, build_memory_maps, build_pes};
use crate::connect::connect_ports;
use crate::types::PlatformConfig;

pub mod builder;
mod connect;
pub mod types;
pub mod yaml;

type ProcessingElements = Vec<Rc<ProcessingElement>>;
type Caches = Vec<Rc<Cache<MemoryAccess>>>;
type Fabrics = Vec<Rc<dyn Fabric<MemoryAccess>>>;
type Memories = Vec<Rc<Memory<MemoryAccess>>>;
type DeviceIds = HashMap<String, DeviceId>;
type NameToIdxMap = HashMap<String, usize>;

#[derive(EntityGet)]
pub struct Platform {
    entity: Rc<Entity>,
    processing_elements: ProcessingElements,
    pes_idx_by_id: NameToIdxMap,
    caches: Caches,
    caches_idx_by_id: NameToIdxMap,
    fabrics: Fabrics,
    fabrics_idx_by_id: NameToIdxMap,
    memories: Memories,
    memories_idx_by_id: NameToIdxMap,
}

impl fmt::Debug for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Platform")
            .field("entity", &self.entity)
            .finish()
    }
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
        let (memories, memories_idx_by_id) = build_memories(engine, clock, top, cfg)?;
        let memory_maps = build_memory_maps(cfg, &memories, &memories_idx_by_id, &device_ids)?;
        let (processing_elements, pes_idx_by_id) =
            build_pes(engine, clock, top, cfg, &memory_maps, &device_ids)?;
        let (caches, caches_idx_by_id) = build_caches(engine, clock, top, cfg)?;
        let (fabrics, fabrics_idx_by_id) = build_fabrics(engine, clock, top, cfg)?;

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

    pub fn dump_stats(&self, time_now_ns: f64) {
        self.dump_memory_totals(time_now_ns);
        self.dump_cache_totals(time_now_ns);
        self.dump_pe_totals(time_now_ns);
        for mem in &self.memories {
            mem.dump_stats(time_now_ns);
        }
        for cache in &self.caches {
            cache.dump_stats(time_now_ns);
        }
        for pe in &self.processing_elements {
            pe.dump_stats(time_now_ns);
        }
    }

    fn dump_memory_totals(&self, time_now_ns: f64) {
        let total_bytes_read: usize = self.memories.iter().map(|mem| mem.bytes_read()).sum();
        let total_bytes_written: usize = self.memories.iter().map(|mem| mem.bytes_written()).sum();

        log_stats(
            &self.entity,
            MemoryStatsDisplay::new(
                "Memory totals",
                time_now_ns,
                total_bytes_read,
                total_bytes_written,
            ),
        );
    }

    fn total_cache_stat<F>(&self, stat_fn: F) -> usize
    where
        F: Fn(&Cache<MemoryAccess>) -> usize,
    {
        self.caches.iter().map(|cache| stat_fn(cache)).sum()
    }

    fn dump_cache_totals(&self, time_now_ns: f64) {
        let total_payload_bytes_read = self.total_cache_stat(Cache::payload_bytes_read);
        let total_payload_bytes_written = self.total_cache_stat(Cache::payload_bytes_written);
        let total_hits = self.total_cache_stat(Cache::num_hits);
        let total_misses = self.total_cache_stat(Cache::num_misses);
        log_stats(
            &self.entity,
            CacheStatsDisplay::new(
                "Cache totals",
                time_now_ns,
                total_payload_bytes_read,
                total_payload_bytes_written,
                total_hits,
                total_misses,
            ),
        );
    }

    fn dump_pe_totals(&self, time_now_ns: f64) {
        let machine_ops: MachineOpCounts =
            self.processing_elements
                .iter()
                .fold(MachineOpCounts::default(), |mut total, pe| {
                    total.add_assign(pe.machine_ops());
                    total
                });
        log_stats(
            &self.entity,
            ProcessingElementStatsDisplay::new(
                "ProcessingElement totals",
                time_now_ns,
                machine_ops,
            ),
        );
    }
}

impl Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.processing_elements.is_empty() {
            writeln!(f, "\nProcessingElements:")?;
            for (i, pe) in self.processing_elements.iter().enumerate() {
                writeln!(f, "  {i}: {}", pe.entity())?;
            }
        }

        if !self.memories.is_empty() {
            writeln!(f, "\nMemories:")?;
            for (i, mem) in self.memories.iter().enumerate() {
                writeln!(f, "  {i}: {}", mem.entity())?;
            }
        }

        if !self.caches.is_empty() {
            writeln!(f, "\nCaches:")?;
            for (i, cache) in self.caches.iter().enumerate() {
                writeln!(f, "  {i}: {}", cache.entity())?;
            }
        }

        if !self.fabrics.is_empty() {
            writeln!(f, "\nFabrics:")?;
            for (i, fabric) in self.fabrics.iter().enumerate() {
                writeln!(f, "  {i}: {}", fabric.entity())?;
            }
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
