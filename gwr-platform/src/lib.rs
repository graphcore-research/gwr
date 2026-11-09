// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

use std::collections::HashMap;
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
use gwr_models::log_stats;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::{Memory, MemoryStatsDisplay};
use gwr_models::model::{CacheModel, FabricModel, MemoryModel};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::model::ProcessingElementModel;
use gwr_models::processing_element::{
    MachineOpCounts, ProcessingElement, ProcessingElementStatsDisplay,
};
use gwr_track::entity::Entity;

use crate::connection_id::fabric_port_endpoint_id;
use crate::instances::{CacheInstance, FabricInstance, MemoryInstance, ProcessingElementInstance};
use crate::types::PlatformConfig;

pub mod builder;
pub mod catalog;
pub mod composition;
mod connect;
pub mod connection_id;
pub mod instances;
mod memory_mapping;
mod prepared;
pub mod rates;
pub use prepared::PreparedPlatform;
pub mod roofline_resources;
pub mod roofline_routes;
pub mod types;
mod validation;
pub mod yaml;

#[derive(EntityGet)]
/// A validated machine topology built from platform configuration.
///
/// `Platform` owns the processing elements, memories, caches, fabrics and
/// port connections that make up the machine. `gwr-platform` is the owner
/// of machine configuration and topology; workload graph structure and
/// execution ordering belong in `gwr-timetable`.
pub struct Platform {
    entity: Rc<Entity>,
    processing_elements: Vec<ProcessingElementInstance>,
    pes_idx_by_id: NameToIdxMap,
    caches: Vec<CacheInstance>,
    caches_idx_by_id: NameToIdxMap,
    coherency_managers: HashMap<String, Rc<CoherencyManager>>,
    fabrics: Vec<FabricInstance>,
    fabrics_idx_by_id: NameToIdxMap,
    memories: Vec<MemoryInstance>,
    memories_idx_by_id: NameToIdxMap,
    device_names_by_id: HashMap<u64, String>,
    atomic_connections: Vec<Vec<connection_id::ConnectionEndpointId>>,
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
        let s = catalog::load_yaml_with_includes(platform_path, catalog::CatalogTarget::Platform)?;
        Platform::from_string(engine, clock, &s)
    }

    pub fn from_string(
        engine: &Engine,
        clock: &Clock,
        platform_config: &str,
    ) -> Result<Self, SimError> {
        catalog::reject_string_includes(platform_config)?;
        let cfg: PlatformConfig = yaml_serde::from_str(platform_config)
            .map_err(|e| SimError(format!("yaml_serde::from_str failed: {e}")))?;
        Platform::build(engine, clock, &cfg)
    }

    fn build(engine: &Engine, clock: &Clock, cfg: &PlatformConfig) -> Result<Self, SimError> {
        PreparedPlatform::new(cfg)?.build(engine, clock)
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

    /// Stable names for per-device native activity snapshots.
    #[must_use]
    pub fn memory_names(&self) -> Vec<String> {
        let mut names = self.memories_idx_by_id.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    #[must_use]
    pub fn cache_names(&self) -> Vec<String> {
        let mut names = self.caches_idx_by_id.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn cache(&self, cache_name: &str) -> Result<&Rc<Cache<MemoryAccess>>, SimError> {
        let idx = self.cache_idx_from_name(cache_name)?;
        self.caches[idx].detailed()
    }

    pub fn cache_idx_from_name(&self, cache_name: &str) -> Result<usize, SimError> {
        match self.caches_idx_by_id.get(cache_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No Cache '{cache_name}'"),
        }
    }

    pub fn fabric(&self, fabric_name: &str) -> Result<&Rc<dyn Fabric<MemoryAccess>>, SimError> {
        let idx = self.fabric_idx_from_name(fabric_name)?;
        self.fabrics[idx].detailed()
    }

    pub fn fabric_idx_from_name(&self, fabric_name: &str) -> Result<usize, SimError> {
        match self.fabrics_idx_by_id.get(fabric_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No Fabric '{fabric_name}'"),
        }
    }

    pub fn validate_atomic_issuer(&self, pe_name: &str, device: u64, address: u64) -> SimResult {
        self.pe(pe_name)?.validate_atomic_memory_binding(
            &self
                .atomic_memory_by_device(device)?
                .atomic_target()
                .unwrap(),
            device,
            address,
        )?;
        let memory = self
            .device_names_by_id
            .get(&device)
            .ok_or_else(|| SimError("unknown atomic target device".into()))?;
        let mut touching = 0usize;
        let mut direct = false;
        for endpoints in &self.atomic_connections {
            if endpoints
                .iter()
                .any(|e| matches!(e,connection_id::ConnectionEndpointId::Pe{name} if name==pe_name))
            {
                touching += 1;
                direct=endpoints.len()==2 && endpoints.iter().any(|e|matches!(e,connection_id::ConnectionEndpointId::Mem{name} if name==memory));
            }
        }
        if touching != 1 || !direct {
            return Err(SimError(
                "atomic assigned PE requires an actual cache-free direct memory connection".into(),
            ));
        }
        Ok(())
    }
    pub fn bind_atomic_task(
        self: &Rc<Self>,
        pe_name: &str,
        id: String,
        device: u64,
        address: u64,
        effect: gwr_models::memory::atomic::Effect,
    ) -> Result<gwr_models::processing_element::task::AtomicTaskConfig, SimError> {
        self.validate_atomic_issuer(pe_name, device, address)?;
        let target = self
            .atomic_memory_by_device(device)?
            .atomic_target()
            .unwrap();
        let config = gwr_models::processing_element::task::AtomicTaskConfig::new(
            id, target, device, address, effect,
        )?
        .with_physical_issuer(self.pe(pe_name)?);
        self.pe(pe_name)?.preflight_atomic_task(&config)?;
        Ok(config)
    }
    pub fn bind_atomic_controlled_task(
        self: &Rc<Self>,
        pe_name: &str,
        id: String,
        device: u64,
        address: u64,
        effect: gwr_models::memory::atomic::Effect,
        controller: Rc<gwr_models::memory::atomic::SplitKController>,
    ) -> Result<gwr_models::processing_element::task::AtomicTaskConfig, SimError> {
        self.validate_atomic_issuer(pe_name, device, address)?;
        let config = gwr_models::processing_element::task::AtomicTaskConfig::new_controlled(
            id,
            self.atomic_memory_by_device(device)?
                .atomic_target()
                .unwrap(),
            device,
            address,
            effect,
            controller,
        )?
        .with_physical_issuer(self.pe(pe_name)?);
        self.pe(pe_name)?.preflight_atomic_task(&config)?;
        Ok(config)
    }
    pub fn atomic_memory_by_device(
        &self,
        device: u64,
    ) -> Result<&Rc<Memory<MemoryAccess>>, SimError> {
        let name = self
            .device_names_by_id
            .get(&device)
            .ok_or_else(|| SimError("unknown atomic target device".into()))?;
        let memory = self.memory(name)?;
        if memory.atomic_target().is_none() {
            return Err(SimError(
                "target has no explicit shared atomic service".into(),
            ));
        }
        Ok(memory)
    }

    pub fn memory(&self, memory_name: &str) -> Result<&Rc<Memory<MemoryAccess>>, SimError> {
        let idx = self.memory_idx_from_name(memory_name)?;
        self.memories[idx].detailed()
    }

    pub fn memory_idx_from_name(&self, memory_name: &str) -> Result<usize, SimError> {
        match self.memories_idx_by_id.get(memory_name) {
            Some(idx) => Ok(*idx),
            None => sim_error!("No Memory '{memory_name}'"),
        }
    }

    pub fn pe(&self, pe_name: &str) -> Result<&Rc<ProcessingElement>, SimError> {
        let idx = self.pe_idx_from_name(pe_name)?;
        self.processing_elements[idx].detailed()
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

    /// Workload-facing PE interface, independent of memory transport fidelity.
    /// The concrete `pe` accessor remains available for detailed model callers.
    pub fn pe_model(&self, pe_name: &str) -> Result<&dyn ProcessingElementModel, SimError> {
        Ok(&*self.processing_elements[self.pe_idx_from_name(pe_name)?])
    }

    pub fn memory_model(&self, name: &str) -> Result<&dyn MemoryModel, SimError> {
        Ok(&*self.memories[self.memory_idx_from_name(name)?])
    }
    pub fn cache_model(&self, name: &str) -> Result<&dyn CacheModel, SimError> {
        Ok(&*self.caches[self.cache_idx_from_name(name)?])
    }
    pub fn fabric_model(&self, name: &str) -> Result<&dyn FabricModel, SimError> {
        Ok(&*self.fabrics[self.fabric_idx_from_name(name)?])
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

    fn total_cache_stat<F>(&self, stat_fn: F) -> usize
    where
        F: Fn(&dyn CacheModel) -> usize,
    {
        self.caches.iter().map(|cache| stat_fn(&**cache)).sum()
    }

    fn dump_cache_totals(&self, time_now_ns: f64) {
        let total_payload_bytes_read = self.total_cache_stat(|cache| cache.payload_bytes_read());
        let total_payload_bytes_written =
            self.total_cache_stat(|cache| cache.payload_bytes_written());
        let total_hits = self.total_cache_stat(|cache| cache.num_hits());
        let total_misses = self.total_cache_stat(|cache| cache.num_misses());
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

type NameToIdxMap = HashMap<String, usize>;

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
    if let Some(connections) = &cfg.connections {
        for connection in connections {
            for endpoint in &connection.connect {
                if let connection_id::ConnectionEndpointId::Pe { name } =
                    connection_id::parse_connection_endpoint_id(endpoint)?
                    && name.strip_suffix(".dma").is_some()
                    && !device_ids.contains_key(&name)
                {
                    device_ids.insert(name, DeviceId(device_id));
                    device_id += 1;
                }
            }
        }
    }
    Ok(device_ids)
}
type DeviceIds = HashMap<String, DeviceId>;

mod fabric_paths;

fn atomic_connections(
    cfg: &PlatformConfig,
) -> Result<Vec<Vec<connection_id::ConnectionEndpointId>>, SimError> {
    cfg.connections
        .iter()
        .flatten()
        .map(|c| {
            c.connect
                .iter()
                .map(|id| connection_id::parse_connection_endpoint_id(id))
                .collect()
        })
        .collect()
}
