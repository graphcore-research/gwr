// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{DeviceId, SimError};
use gwr_models::memory::memory_map::MemoryMap;
use gwr_track::entity::Entity;

use crate::builder::{
    EffectiveConfigs, build_caches_from_configs, build_fabric_destination_port_maps,
    build_fabrics_from_configs, build_memories_from_configs, build_pes_from_configs,
    fabric_configs_with_destination_port_maps,
};
use crate::connect::connect_ports;
use crate::roofline_resources::ResourceCatalog;
use crate::roofline_routes::RooflineRoutes;
use crate::types::PlatformConfig;
use crate::{DeviceIds, Platform, assign_device_ids};

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
    memory_maps: HashMap<String, Rc<MemoryMap>>,
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
        let mut effective = cfg.validated()?;
        if timing {
            eprintln!(
                "roofline timing: prepared_validate={:.3}s",
                phase_started.elapsed().as_secs_f64()
            );
        }
        phase_started = std::time::Instant::now();
        let device_ids = assign_device_ids(cfg)?;
        let mut ranges = HashMap::new();
        for (memory, config) in cfg.memories.iter().flatten().zip(&effective.memories) {
            ranges.insert(memory.name.as_str(), config.address_range()?);
        }
        let mut memory_maps = HashMap::new();
        for section in &cfg.memory_maps {
            let mut members = Vec::new();
            for device in &section.devices {
                let range = ranges
                    .get(device.as_str())
                    .ok_or_else(|| SimError(format!("Unknown memory '{device}'")))?;
                let id = device_ids
                    .get(device)
                    .ok_or_else(|| SimError(format!("Unknown device '{device}'")))?;
                members.push((
                    device.clone(),
                    gwr_models::memory::memory_map::MemoryRegion {
                        start: *range.start(),
                        end: *range.end(),
                        device: *id,
                    },
                ));
            }
            let map = crate::memory_mapping::build_map(section, members)?;
            memory_maps.insert(section.name.clone(), Rc::new(map));
        }
        if timing {
            eprintln!(
                "roofline timing: prepared_memory_maps={:.3}s",
                phase_started.elapsed().as_secs_f64()
            );
        }
        phase_started = std::time::Instant::now();
        let routes = build_fabric_destination_port_maps(cfg, &device_ids)?;
        if timing {
            eprintln!(
                "roofline timing: prepared_fabric_maps={:.3}s",
                phase_started.elapsed().as_secs_f64()
            );
        }
        phase_started = std::time::Instant::now();
        effective.fabrics = fabric_configs_with_destination_port_maps(cfg, &routes)?;
        if timing {
            eprintln!(
                "roofline timing: prepared_effective_fabrics={:.3}s total={:.3}s",
                phase_started.elapsed().as_secs_f64(),
                total_started.elapsed().as_secs_f64()
            );
        }
        Ok(Self {
            cfg,
            effective,
            device_ids,
            memory_maps,
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
        self.memory_maps
            .get(name)
            .ok_or_else(|| SimError(format!("Unknown memory map '{name}'")))
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
            memory_maps,
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
            &memory_maps,
            &device_ids,
        )?;
        let (caches, caches_idx_by_id) = build_caches_from_configs(
            engine,
            clock,
            top,
            cfg,
            &effective.caches,
            &memory_maps,
            &device_ids,
        )?;
        let coherency_managers = crate::builder::build_coherency_managers(
            engine,
            clock,
            top,
            cfg,
            &memory_maps,
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
