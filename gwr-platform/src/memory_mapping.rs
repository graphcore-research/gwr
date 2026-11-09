// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Shared map construction and stripe validation, before simulator allocation.
use std::collections::{HashMap, HashSet};

use gwr_engine::types::{DeviceId, SimError, SimResult};
use gwr_models::memory::memory_map::{MemoryMap, MemoryRegion};

use crate::builder::EffectiveConfigs;
use crate::connection_id::{CachePortId, ConnectionEndpointId, parse_connection_endpoint_id};
use crate::types::{AddressMappingMode, MemoryMapSection, PlatformConfig};

pub(crate) fn physical_end(start: u64, size: u64) -> Result<u64, SimError> {
    gwr_models::memory::checked_last_address(start, size)
        .ok_or_else(|| SimError("Invalid physical memory range".into()))
}

pub(crate) fn build_map(
    cfg: &MemoryMapSection,
    members: Vec<(String, MemoryRegion)>,
) -> Result<MemoryMap, SimError> {
    let mut map = MemoryMap::new();
    let mut grouped = HashSet::new();
    if let Some(policy) = &cfg.address_mapping {
        let error = |message: &str| SimError(format!("Memory map '{}': {message}", cfg.name));
        let mut names = HashSet::new();
        for group in &policy.groups {
            if group.name.trim().is_empty() || !names.insert(&group.name) {
                return Err(error("stripe group names must be nonempty and unique"));
            }
            let mut regions = Vec::new();
            for name in &group.devices {
                if !grouped.insert(name.clone()) {
                    return Err(error("duplicate memory in stripe groups"));
                }
                let (_, region) = members
                    .iter()
                    .find(|(n, _)| n == name)
                    .ok_or_else(|| error(&format!("unknown stripe group memory '{name}'")))?;
                regions.push(region.clone());
            }
            // Validate declared groups in BOTH modes, without changing off-mode
            // placement.
            let target = if policy.mode == AddressMappingMode::Striped {
                &mut map
            } else {
                &mut MemoryMap::new()
            };
            target
                .insert_striped(regions, policy.stripe_bytes)
                .map_err(|e| error(&format!("stripe group '{}': {e}", group.name)))?;
        }
        if !policy.stripe_bytes.is_power_of_two() || policy.groups.is_empty() {
            return Err(error(
                "require a power-of-two stripe_bytes and nonempty groups",
            ));
        }
    }
    for (name, region) in members {
        if cfg
            .address_mapping
            .as_ref()
            .is_some_and(|p| p.mode == AddressMappingMode::Striped)
            && grouped.contains(&name)
        {
            continue;
        }
        let size = u128::from(region.end) + 1 - u128::from(region.start);
        let size = u64::try_from(size).map_err(|_| SimError("Physical window too large".into()))?;
        map.insert(region.start, size, region.device)?;
    }
    Ok(map)
}

pub(crate) fn validate(platform: &PlatformConfig, effective: &EffectiveConfigs) -> SimResult {
    let physical: HashMap<_, _> = platform
        .memories
        .iter()
        .flatten()
        .zip(&effective.memories)
        .enumerate()
        .map(|(i, (m, c))| {
            Ok((
                m.name.as_str(),
                MemoryRegion {
                    start: m.base_address,
                    end: *c.address_range()?.end(),
                    device: DeviceId(i as u64),
                },
            ))
        })
        .collect::<Result<_, SimError>>()?;
    let graph = request_graph(platform)?;
    for map in &platform.memory_maps {
        if map.address_mapping.is_none() {
            continue;
        }
        let members = map
            .devices
            .iter()
            .map(|name| {
                Ok((
                    name.clone(),
                    physical
                        .get(name.as_str())
                        .ok_or_else(|| SimError(format!("Unknown memory '{name}'")))?
                        .clone(),
                ))
            })
            .collect::<Result<_, SimError>>()?;
        build_map(map, members)?;
        let stripe = map.address_mapping.as_ref().unwrap().stripe_bytes;
        for (pe, config) in platform
            .processing_elements
            .iter()
            .flatten()
            .zip(&effective.processing_elements)
        {
            for (interface, selected) in [
                (pe.name.clone(), pe.memory_map.as_str()),
                (
                    format!("{}.dma", pe.name),
                    pe.dma_memory_map.as_deref().unwrap_or(&pe.memory_map),
                ),
            ] {
                if selected != map.name {
                    continue;
                }
                validate_fetch(&map.name, stripe, config.lsu_access_bytes, &interface)?;
                let mut pending = vec![format!("pe.{interface}")];
                let mut seen = HashSet::new();
                while let Some(node) = pending.pop() {
                    if !seen.insert(node.clone()) {
                        continue;
                    }
                    for (cache, cache_config) in
                        platform.caches.iter().flatten().zip(&effective.caches)
                    {
                        if node == format!("cache.{}.dev", cache.name) {
                            validate_fetch(
                                &map.name,
                                stripe,
                                cache_config.line_size_bytes(),
                                &cache.name,
                            )?;
                        }
                    }
                    pending.extend(graph.get(&node).into_iter().flatten().cloned());
                }
            }
        }
    }
    Ok(())
}

fn validate_fetch(map: &str, stripe: u64, bytes: usize, component: &str) -> SimResult {
    if bytes == 0 || !stripe.is_multiple_of(bytes as u64) {
        return Err(SimError(format!(
            "Memory map '{map}': {stripe}-byte stripe incompatible with {bytes}-byte fetch at '{component}'"
        )));
    }
    Ok(())
}

// External links are bidirectional, but requests only traverse caches dev→mem.
// Collapse fabric ports to their fabric: routed destinations may use any
// egress.
fn request_graph(platform: &PlatformConfig) -> Result<HashMap<String, Vec<String>>, SimError> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    for cache in platform.caches.iter().flatten() {
        graph
            .entry(format!("cache.{}.dev", cache.name))
            .or_default()
            .push(format!("cache.{}.mem", cache.name));
    }
    for connection in platform.connections.iter().flatten() {
        let [a, b] = connection.connect.as_slice() else {
            continue;
        };
        let a = parse_connection_endpoint_id(a)?;
        let b = parse_connection_endpoint_id(b)?;
        let node = |endpoint: &ConnectionEndpointId, peer: &ConnectionEndpointId, left: bool| {
            match endpoint {
                ConnectionEndpointId::Null => None,
                ConnectionEndpointId::FabricPort { fabric, .. } => Some(format!("fabric.{fabric}")),
                ConnectionEndpointId::Cache { name, port } => {
                    let inferred = match peer {
                        ConnectionEndpointId::Pe { .. } => CachePortId::Dev,
                        ConnectionEndpointId::Cache { .. } if !left => CachePortId::Dev,
                        _ => CachePortId::Mem,
                    };
                    let side = if port.as_ref().unwrap_or(&inferred) == &CachePortId::Dev {
                        "dev"
                    } else {
                        "mem"
                    };
                    Some(format!("cache.{name}.{side}"))
                }
                other => Some(other.to_string()),
            }
        };
        if let (Some(a), Some(b)) = (node(&a, &b, true), node(&b, &a, false)) {
            graph.entry(a.clone()).or_default().push(b.clone());
            graph.entry(b).or_default().push(a);
        }
    }
    Ok(graph)
}
