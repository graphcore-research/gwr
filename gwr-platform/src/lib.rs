// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display};
use std::path::Path;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{DeviceId, SimError, SimResult};
use gwr_model_builder::EntityGet;
use gwr_models::cache::coherency_manager::CoherencyManager;
use gwr_models::cache::{Cache, CacheStatsDisplay};
use gwr_models::fabric::Fabric;
use gwr_models::fabric::functional::FunctionalFabric;
use gwr_models::fabric::routed::RoutedFabric;
use gwr_models::log_stats;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::{Memory, MemoryStatsDisplay};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::{
    MachineOpCounts, ProcessingElement, ProcessingElementStatsDisplay,
};
use gwr_track::entity::{Entity, GetEntity};

use crate::connect::connect_ports;
use crate::connection_id::fabric_port_endpoint_id;
use crate::resolved::ResolvedFabric;
use crate::types::{FabricKind, PlatformConfig};

pub mod builder;
mod connect;
pub mod connection_id;
mod resolved;
pub mod types;
mod validation;
pub mod yaml;

type ProcessingElements = Vec<Rc<ProcessingElement>>;
type Caches = Vec<Rc<Cache<MemoryAccess>>>;
type Fabrics = Vec<Rc<dyn Fabric<MemoryAccess>>>;
type Memories = Vec<Rc<Memory<MemoryAccess>>>;
type DeviceIds = HashMap<String, DeviceId>;
type NameToIdxMap = HashMap<String, usize>;

#[derive(EntityGet)]
/// A validated machine topology built from platform configuration.
///
/// `Platform` owns the processing elements, memories, caches, fabrics and
/// port connections that make up the machine. `gwr-platform` is the owner
/// of machine configuration and topology; workload graph structure and
/// execution ordering belong in `gwr-timetable`.
pub struct Platform {
    entity: Rc<Entity>,
    processing_elements: ProcessingElements,
    pes_idx_by_id: NameToIdxMap,
    caches: Caches,
    caches_idx_by_id: NameToIdxMap,
    coherency_managers: HashMap<String, Rc<CoherencyManager>>,
    fabrics: Fabrics,
    fabrics_idx_by_id: NameToIdxMap,
    memories: Memories,
    memories_idx_by_id: NameToIdxMap,
    device_names_by_id: HashMap<u64, String>,
    reachable_memories: Vec<HashSet<DeviceId>>,
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
        let cfg: PlatformConfig = yaml_serde::from_str(platform_config)
            .map_err(|e| SimError(format!("yaml_serde::from_str failed: {e}")))?;
        Platform::from_config(engine, clock, &cfg)
    }

    /// Build a platform from an already parsed configuration.
    ///
    /// Validates effective settings, references and routing before registering
    /// runtime models and connecting their ports.
    ///
    /// # Errors
    ///
    /// Returns an error if configuration validation or model construction
    /// fails.
    pub fn from_config(
        engine: &Engine,
        clock: &Clock,
        cfg: &PlatformConfig,
    ) -> Result<Self, SimError> {
        let resolved = cfg.resolve()?;
        let top = engine.top();
        let memories = resolved
            .memories
            .into_iter()
            .map(|memory| Memory::new_and_register(engine, clock, top, &memory.name, memory.config))
            .collect::<Result<Vec<_>, _>>()?;
        let processing_elements = resolved
            .pes
            .into_iter()
            .map(|pe| {
                ProcessingElement::new_and_register(
                    engine,
                    clock,
                    top,
                    &pe.name,
                    &pe.memory_map,
                    &pe.config,
                    pe.device_id,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let caches = resolved
            .caches
            .into_iter()
            .map(|cache| match cache.coherence {
                Some(coherence) => Cache::new_coherent_and_register(
                    engine,
                    clock,
                    top,
                    &cache.name,
                    cache.config,
                    coherence,
                ),
                None => Cache::new_and_register(engine, clock, top, &cache.name, cache.config),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let managers = resolved
            .managers
            .into_iter()
            .map(|(name, config)| {
                CoherencyManager::new_and_register(engine, clock, top, &name, config)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let coherency_managers = managers
            .iter()
            .map(|manager| (manager.entity().name.clone(), manager.clone()))
            .collect();
        let fabrics = resolved
            .fabrics
            .into_iter()
            .map(|fabric| register_fabric(engine, clock, fabric))
            .collect::<Result<Vec<_>, _>>()?;
        let pes_idx_by_id = processing_elements
            .iter()
            .enumerate()
            .map(|(index, pe)| (pe.entity().name.clone(), index))
            .collect();
        let memories_idx_by_id = memories
            .iter()
            .enumerate()
            .map(|(index, memory)| (memory.entity().name.clone(), index))
            .collect();
        let caches_idx_by_id = caches
            .iter()
            .enumerate()
            .map(|(index, cache)| (cache.entity().name.clone(), index))
            .collect();
        let fabrics_idx_by_id = fabrics
            .iter()
            .enumerate()
            .map(|(index, fabric)| (fabric.entity().name.clone(), index))
            .collect();

        let parent = engine.top();
        let entity = Rc::new(Entity::new(parent, "platform"));
        let platform = Platform {
            entity,
            processing_elements,
            pes_idx_by_id,
            caches,
            caches_idx_by_id,
            coherency_managers,
            fabrics,
            fabrics_idx_by_id,
            memories,
            memories_idx_by_id,
            device_names_by_id: resolved.device_names_by_id,
            reachable_memories: resolved.reachable_memories,
        };
        connect_ports(engine, clock, &platform, &managers, &resolved.connections)?;
        Ok(platform)
    }

    /// Mapped physical devices reachable from a PE through platform
    /// connections.
    ///
    /// # Errors
    ///
    /// Returns an error if `pe_name` does not identify a processing element.
    pub fn reachable_memories(&self, pe_name: &str) -> Result<&HashSet<DeviceId>, SimError> {
        Ok(&self.reachable_memories[self.pe_idx_from_name(pe_name)?])
    }

    pub fn coherency_manager(&self, name: &str) -> Result<&Rc<CoherencyManager>, SimError> {
        self.coherency_managers
            .get(name)
            .ok_or_else(|| SimError(format!("No Coherency Manager '{name}'")))
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

    pub fn cache_idx_from_name(&self, cache_name: &str) -> Result<usize, SimError> {
        match self.caches_idx_by_id.get(cache_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No Cache '{cache_name}'"),
        }
    }

    pub fn fabric(&self, fabric_name: &str) -> Result<&Rc<dyn Fabric<MemoryAccess>>, SimError> {
        let idx = self.fabric_idx_from_name(fabric_name)?;
        Ok(&self.fabrics[idx])
    }

    pub fn fabric_idx_from_name(&self, fabric_name: &str) -> Result<usize, SimError> {
        match self.fabrics_idx_by_id.get(fabric_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No Fabric '{fabric_name}'"),
        }
    }

    pub fn memory(&self, memory_name: &str) -> Result<&Rc<Memory<MemoryAccess>>, SimError> {
        let idx = self.memory_idx_from_name(memory_name)?;
        Ok(&self.memories[idx])
    }

    pub fn memory_idx_from_name(&self, memory_name: &str) -> Result<usize, SimError> {
        match self.memories_idx_by_id.get(memory_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No Memory '{memory_name}'"),
        }
    }

    pub fn pe(&self, pe_name: &str) -> Result<&Rc<ProcessingElement>, SimError> {
        let idx = self.pe_idx_from_name(pe_name)?;
        Ok(&self.processing_elements[idx])
    }

    pub fn pe_idx_from_name(&self, pe_name: &str) -> Result<usize, SimError> {
        match self.pes_idx_by_id.get(pe_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No PE '{pe_name}'"),
        }
    }

    pub fn attach_dispatcher(&self, dispatcher: &Rc<dyn Dispatch>) {
        for pe in &self.processing_elements {
            pe.set_dispatcher(dispatcher);
        }
    }

    pub fn try_dump_stats(&self, time_now_ns: f64) -> SimResult {
        let (machine_ops, bytes_consumed, bytes_produced) = self.processing_element_totals()?;
        self.dump_memory_totals(time_now_ns);
        self.dump_cache_totals(time_now_ns);
        self.dump_pe_totals(time_now_ns, machine_ops, bytes_consumed, bytes_produced);
        for manager in self.coherency_managers.values() {
            manager.dump_stats(time_now_ns);
        }
        for mem in &self.memories {
            mem.dump_stats(time_now_ns);
        }
        for cache in &self.caches {
            cache.dump_stats(time_now_ns);
        }
        for pe in &self.processing_elements {
            pe.dump_stats(time_now_ns);
        }
        Ok(())
    }

    #[must_use]
    pub fn fabric_port_map_description(&self) -> String {
        let mut out = String::new();
        for fabric in &self.fabrics {
            use std::fmt::Write as _;

            let _ = writeln!(
                out,
                "Fabric {} port_selection: {}",
                fabric.entity().name,
                fabric.port_selection()
            );
            let mut entries: Vec<_> = fabric.destination_port_map().iter().collect();
            entries.sort_by_key(|(device_id, _)| **device_id);
            for (device_id, ports) in entries {
                let device_name = self
                    .device_names_by_id
                    .get(device_id)
                    .map_or("<unknown>", String::as_str);
                let port_strings: Vec<String> = ports
                    .iter()
                    .map(|port_idx| {
                        let (col, row, port) = fabric.fabric_port_index_to_col_row_port(*port_idx);
                        let endpoint =
                            fabric_port_endpoint_id(fabric.entity().name.as_str(), col, row, port);
                        format!("{port_idx}={endpoint} ({col},{row}).{port}")
                    })
                    .collect();
                let _ = writeln!(
                    out,
                    "  device {device_id} ({device_name}) -> {}",
                    port_strings.join(", ")
                );
            }
        }
        out
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

    fn total_cache_stat<F>(&self, stat_fn: F) -> usize
    where
        F: Fn(&Cache<MemoryAccess>) -> usize,
    {
        self.caches.iter().map(|cache| stat_fn(cache)).sum()
    }

    fn processing_element_totals(&self) -> Result<(MachineOpCounts, usize, usize), SimError> {
        self.processing_elements.iter().try_fold(
            (MachineOpCounts::default(), 0usize, 0usize),
            |total, pe| {
                Ok((
                    total.0.checked_add(pe.machine_ops())?,
                    total
                        .1
                        .checked_add(pe.bytes_consumed())
                        .ok_or_else(|| SimError("PE consumed byte total overflows".to_string()))?,
                    total
                        .2
                        .checked_add(pe.bytes_produced())
                        .ok_or_else(|| SimError("PE produced byte total overflows".to_string()))?,
                ))
            },
        )
    }

    fn dump_pe_totals(
        &self,
        time_now_ns: f64,
        machine_ops: MachineOpCounts,
        bytes_consumed: usize,
        bytes_produced: usize,
    ) {
        log_stats(
            &self.entity,
            ProcessingElementStatsDisplay::new(
                "ProcessingElement totals",
                time_now_ns,
                machine_ops,
            )
            .with_data(bytes_consumed, bytes_produced),
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

fn register_fabric(
    engine: &Engine,
    clock: &Clock,
    fabric: ResolvedFabric,
) -> Result<Rc<dyn Fabric<MemoryAccess>>, SimError> {
    let top = engine.top();
    let model: Rc<dyn Fabric<MemoryAccess>> = match fabric.kind {
        FabricKind::Functional => {
            FunctionalFabric::new_and_register(engine, clock, top, &fabric.name, fabric.config)?
        }
        FabricKind::Routed => RoutedFabric::new_and_register(
            engine,
            clock,
            top,
            &fabric.name,
            fabric.config,
            fabric.routing,
        )?,
    };
    Ok(model)
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
    for name in cfg
        .caches
        .iter()
        .flatten()
        .filter(|cache| cache.coherency_managers.is_some())
        .map(|cache| &cache.name)
        .chain(
            cfg.coherency_managers
                .iter()
                .flatten()
                .map(|manager| &manager.name),
        )
    {
        if device_ids
            .insert(name.clone(), DeviceId(device_id))
            .is_some()
        {
            return sim_error!("Duplicate device name {name}");
        }
        device_id += 1;
    }
    Ok(device_ids)
}
