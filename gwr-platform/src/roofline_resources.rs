// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Engine-free capacity mapping for aggregate roofline analysis.
//! This catalog does not select paths, group domains, or register models.

use std::collections::BTreeMap;

use gwr_engine::types::SimError;

use crate::builder::EffectiveConfigs;
use crate::types::{FabricKind, PlatformConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceKind {
    PeCompute,
    MemoryPayload,
    CacheRequest,
    CacheMemoryResponse,
    /// Per-plane local delivery service of a routed fabric.
    FabricPlaneDelivery {
        plane: usize,
        port: usize,
    },
    /// Per-plane directed mesh link.
    FabricPlaneLink {
        plane: usize,
        from: (usize, usize),
        to: (usize, usize),
    },
    FabricIngress {
        port: usize,
    },
    FabricEgress {
        port: usize,
    },
    /// Directed adjacent mesh link. Coordinates are (column, row).
    FabricLink {
        from: (usize, usize),
        to: (usize, usize),
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityUnit {
    BytesPerTick,
    ServiceTicksPerTick,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceDescriptor {
    pub component: String,
    pub kind: ResourceKind,
    /// Per tick of the platform's single simulation clock.
    pub capacity: f64,
}

impl ResourceDescriptor {
    #[must_use]
    pub fn unit(&self) -> CapacityUnit {
        match self.kind {
            ResourceKind::PeCompute => CapacityUnit::ServiceTicksPerTick,
            _ => CapacityUnit::BytesPerTick,
        }
    }
}

/// Immutable indexed resources, ordered by PE/cache/memory/fabric declaration
/// order, then local resource order. Indices are not persistent across edits.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceCatalog {
    resources: Vec<ResourceDescriptor>,
    by_component: BTreeMap<String, BTreeMap<ResourceKind, usize>>,
}

impl ResourceCatalog {
    pub(crate) fn from_configs(
        cfg: &PlatformConfig,
        effective: &EffectiveConfigs,
    ) -> Result<Self, SimError> {
        let mut catalog = Self {
            resources: Vec::new(),
            by_component: BTreeMap::new(),
        };
        for pe in cfg.processing_elements.iter().flatten() {
            catalog.add(&pe.name, ResourceKind::PeCompute, 1.)?;
        }
        for (cache, config) in cfg.caches.iter().flatten().zip(&effective.caches) {
            for kind in [
                ResourceKind::CacheRequest,
                ResourceKind::CacheMemoryResponse,
            ] {
                catalog.add(&cache.name, kind, config.bw_bytes_per_tick() as f64)?;
            }
        }
        for (memory, config) in cfg.memories.iter().flatten().zip(&effective.memories) {
            catalog.add(
                &memory.name,
                ResourceKind::MemoryPayload,
                config.bw_bytes_per_tick() as f64,
            )?;
        }
        for (fabric, (config, _)) in cfg.fabrics.iter().flatten().zip(&effective.fabrics) {
            if matches!(fabric.kind, FabricKind::Functional) {
                let capacity = config.port_bits_per_tick() as f64 / 8.;
                for &port in config.port_indices() {
                    catalog.add(&fabric.name, ResourceKind::FabricIngress { port }, capacity)?;
                    catalog.add(&fabric.name, ResourceKind::FabricEgress { port }, capacity)?;
                }
                continue;
            }
            let paths = fabric.config.paths.clone().unwrap_or_default();
            paths.validate()?;
            let capacity = paths.bw_bits_per_tick as f64 / 8.;
            for plane in 0..paths.count {
                for &port in config.port_indices() {
                    catalog.add(
                        &fabric.name,
                        delivery_kind(paths.count, plane, port),
                        capacity,
                    )?;
                }
                for column in 0..config.num_columns() {
                    for row in 0..config.num_rows() {
                        let from = (column, row);
                        for to in [
                            (
                                column.checked_add(1).filter(|&c| c < config.num_columns()),
                                Some(row),
                            ),
                            (
                                Some(column),
                                row.checked_add(1).filter(|&r| r < config.num_rows()),
                            ),
                        ] {
                            if let (Some(c), Some(r)) = to {
                                let to = (c, r);
                                catalog.add(
                                    &fabric.name,
                                    link_kind(paths.count, plane, from, to),
                                    capacity,
                                )?;
                                catalog.add(
                                    &fabric.name,
                                    link_kind(paths.count, plane, to, from),
                                    capacity,
                                )?;
                            }
                        }
                    }
                }
            }
        }
        Ok(catalog)
    }

    fn add(&mut self, name: &str, kind: ResourceKind, capacity: f64) -> Result<(), SimError> {
        if !capacity.is_finite() || capacity <= 0. {
            return Err(SimError(format!(
                "invalid roofline capacity for '{name}' {kind:?}"
            )));
        }
        let kinds = self.by_component.entry(name.into()).or_default();
        if kinds.insert(kind, self.resources.len()).is_some() {
            return Err(SimError(format!(
                "duplicate roofline resource for '{name}' {kind:?}"
            )));
        }
        self.resources.push(ResourceDescriptor {
            component: name.into(),
            kind,
            capacity,
        });
        Ok(())
    }

    #[must_use]
    pub fn resources(&self) -> &[ResourceDescriptor] {
        &self.resources
    }

    #[must_use]
    pub fn index(&self, component: &str, kind: ResourceKind) -> Option<usize> {
        self.by_component.get(component)?.get(&kind).copied()
    }

    /// Capacities in exactly the order used by `index`. The caller must use
    /// this same table for its service plans and scheduler/domain.
    #[must_use]
    pub fn capacities(&self) -> Vec<f64> {
        self.resources
            .iter()
            .map(|resource| resource.capacity)
            .collect()
    }
}

// Retain the original names for a single plane; multiple planes always have
// explicitly qualified resource identities.
pub(crate) fn delivery_kind(count: usize, plane: usize, port: usize) -> ResourceKind {
    if count == 1 {
        ResourceKind::FabricEgress { port }
    } else {
        ResourceKind::FabricPlaneDelivery { plane, port }
    }
}
pub(crate) fn link_kind(
    count: usize,
    plane: usize,
    from: (usize, usize),
    to: (usize, usize),
) -> ResourceKind {
    if count == 1 {
        ResourceKind::FabricLink { from, to }
    } else {
        ResourceKind::FabricPlaneLink { plane, from, to }
    }
}
