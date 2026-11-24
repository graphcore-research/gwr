// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{DeviceId, SimError, SimResult};
use gwr_models::memory::memory_map::MemoryMap;
use gwr_track::entity::Entity;

use crate::builder::{
    EffectiveConfigs, build_caches_from_configs, build_fabrics_from_configs,
    build_memories_from_configs, build_pes_from_configs,
};
use crate::connect::connect_ports;
use crate::memory_mapping::AddressMaps;
use crate::roofline_resources::ResourceCatalog;
use crate::roofline_routes::RooflineRoutes;
use crate::types::{PlatformConfig, ProcessingElementSection};
use crate::{DeviceIds, Platform};

/// Validated topology and address metadata, resolved without constructing
/// models.
///
/// The immutable configuration borrow prevents edits between preflight and
/// construction. Device IDs retain platform declaration order (PEs, then
/// memories). Address maps do not depend on the fidelity of a memory model.
/// Construction supports the detailed platform runtime. Analytical roofline
/// analysis uses the same validated physical configuration independently.
pub struct PreparedPlatform<'a> {
    cfg: &'a PlatformConfig,
    effective: EffectiveConfigs,
    device_ids: DeviceIds,
    maps: AddressMaps,
}

impl<'a> PreparedPlatform<'a> {
    /// Compile deterministic analysis routes without activating models.
    /// Multi-candidate hash routes remain unsupported by this compiler stage.
    pub fn roofline_routes(&self) -> Result<RooflineRoutes<'_>, SimError> {
        RooflineRoutes::new(self.cfg, &self.effective, &self.device_ids)
    }

    /// Compile analysis capacities from physical component configuration
    /// without constructing models or registering engine work.
    pub fn roofline_resources(&self) -> Result<ResourceCatalog, SimError> {
        ResourceCatalog::from_configs(self.cfg, &self.effective)
    }

    /// Validate configuration, resolve address maps and compile fabric routes.
    /// No engine, runnable registration or trace entity is created here.
    pub fn new(cfg: &'a PlatformConfig) -> Result<Self, SimError> {
        let timing = std::env::var_os("GWR_ROOFLINE_TIMINGS").is_some();
        let total_started = std::time::Instant::now();
        let mut phase_started = total_started;
        let (mut effective, device_ids, maps) = cfg.validated()?;
        if timing {
            eprintln!(
                "roofline timing: prepared_validate={:.3}s",
                phase_started.elapsed().as_secs_f64()
            );
        }
        phase_started = std::time::Instant::now();
        effective.install_fabric_routes(cfg, &device_ids)?;
        if timing {
            eprintln!(
                "roofline timing: prepared_fabric_maps={:.3}s total={:.3}s",
                phase_started.elapsed().as_secs_f64(),
                total_started.elapsed().as_secs_f64()
            );
        }
        Ok(Self {
            cfg,
            effective,
            device_ids,
            maps,
        })
    }

    /// Look up the stable device identity used by address maps and routing.
    pub fn device_id(&self, name: &str) -> Result<DeviceId, SimError> {
        self.device_ids
            .get(name)
            .copied()
            .ok_or_else(|| SimError(format!("Unknown device '{name}'")))
    }

    /// Inspect or share a resolved address map without building memory models.
    pub fn memory_map(&self, name: &str) -> Result<&Rc<MemoryMap>, SimError> {
        self.maps
            .memory_maps
            .get(name)
            .ok_or_else(|| SimError(format!("Unknown memory map '{name}'")))
    }

    /// Inspect the PE's resolved logical map, including named virtual stripes.
    pub fn pe_memory_map(&self, pe: &ProcessingElementSection) -> Result<&Rc<MemoryMap>, SimError> {
        crate::memory_mapping::pe_map(pe, &self.maps.memory_maps, &self.maps.address_spaces)
    }

    /// Find physical memories connected to the PE's compute request port.
    pub fn reachable_memories(&self, pe_name: &str) -> Result<HashSet<DeviceId>, SimError> {
        if !self
            .cfg
            .processing_elements
            .iter()
            .flatten()
            .any(|pe| pe.name == pe_name)
        {
            return Err(SimError(format!("Unknown PE '{pe_name}'")));
        }
        let endpoint = format!("pe.{pe_name}");
        Ok(
            crate::memory_mapping::reachable_request_nodes(&self.maps.request_graph, &endpoint)
                .into_iter()
                .filter_map(|node| node.strip_prefix("mem."))
                .map(|name| self.device_ids[name])
                .collect(),
        )
    }

    /// Require one placement per physical memory for workloads sharing
    /// virtual addresses across PEs.
    pub fn validate_shared_address_mappings(&self) -> SimResult {
        let mut mappings = HashMap::new();
        for pe in self.cfg.processing_elements.iter().flatten() {
            let name = pe.address_space.as_deref().unwrap_or(&pe.memory_map);
            for (region, placement) in self.pe_memory_map(pe)?.physical_mappings() {
                if let Some((previous_name, previous)) = mappings.get(&region.device) {
                    if *previous != placement {
                        let memory = self
                            .device_ids
                            .iter()
                            .find_map(|(name, id)| (*id == region.device).then_some(name.as_str()))
                            .expect("validated device identity");
                        return Err(SimError(format!(
                            "incompatible address mappings for memory '{memory}' in '{previous_name}' and '{name}'"
                        )));
                    }
                } else {
                    mappings.insert(region.device, (name, placement));
                }
            }
        }
        Ok(())
    }

    /// Construct and connect the validated platform on the supplied engine
    /// clock.
    ///
    /// Registrations become executable only after every model and connection
    /// succeeds. Failure rolls back this build, preserving existing engine
    /// work. Nested registration transactions are rejected. Entity IDs and
    /// trace construction records are not rolled back.
    pub fn build(self, engine: &Engine, clock: &Clock) -> Result<Platform, SimError> {
        let transaction = engine.registration_transaction()?;
        let Self {
            cfg,
            effective,
            device_ids,
            maps,
        } = self;
        let top = engine.top();
        let (memories, memories_idx_by_id) =
            build_memories_from_configs(engine, clock, top, cfg, &effective.memories)?;
        let (processing_elements, pes_idx_by_id) = build_pes_from_configs(
            engine,
            clock,
            top,
            cfg,
            &effective.processing_elements,
            &maps.memory_maps,
            &maps.address_spaces,
            &device_ids,
        )?;
        let (caches, caches_idx_by_id) = build_caches_from_configs(
            engine,
            clock,
            top,
            cfg,
            &effective.caches,
            &maps.memory_maps,
            &device_ids,
        )?;
        let coherency_managers = crate::builder::build_coherency_managers(
            engine,
            clock,
            top,
            cfg,
            &maps.memory_maps,
            &device_ids,
        )?;
        let (fabrics, fabrics_idx_by_id) =
            build_fabrics_from_configs(engine, clock, top, cfg, &effective.fabrics)?;
        let device_names_by_id = device_ids
            .iter()
            .map(|(name, device_id)| (device_id.0, name.clone()))
            .collect();
        let entity = Rc::new(Entity::new(top, "platform"));
        let platform = Platform {
            entity,
            processing_elements: processing_elements.into_iter().map(Into::into).collect(),
            pes_idx_by_id,
            caches: caches.into_iter().map(Into::into).collect(),
            caches_idx_by_id,
            coherency_managers,
            fabrics: fabrics.into_iter().map(Into::into).collect(),
            fabrics_idx_by_id,
            memories: memories.into_iter().map(Into::into).collect(),
            memories_idx_by_id,
            device_names_by_id,
            atomic_connections: crate::atomic_connections(cfg)?,
        };
        connect_ports(engine, clock, &platform, cfg)?;
        transaction.commit();
        Ok(platform)
    }
}
