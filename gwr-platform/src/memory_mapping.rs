// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Shared map construction and stripe validation, before simulator allocation.
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::rc::Rc;

use gwr_engine::types::{SimError, SimResult};
use gwr_models::memory::memory_map::{MemoryMap, MemoryRegion};
use gwr_models::processing_element::address_space::{AddressRegionPolicy, AddressSpace};

use crate::DeviceIds;
use crate::builder::EffectiveConfigs;
use crate::connection_id::{ConnectionEndpointId, parse_connection_endpoint_id};
use crate::types::{
    AddressMappingMode, AddressRegionSection, AddressSpaceSection, MemoryMapSection,
    PlatformConfig, ProcessingElementSection,
};
use crate::validation::cache_connection_port;

pub(crate) struct AddressMaps {
    pub(crate) memory_maps: HashMap<String, Rc<MemoryMap>>,
    pub(crate) address_spaces: HashMap<String, Rc<MemoryMap>>,
    pub(crate) request_graph: HashMap<String, Vec<String>>,
}

pub(crate) fn pe_map<'a, S: BuildHasher>(
    pe: &ProcessingElementSection,
    memory_maps: &'a HashMap<String, Rc<MemoryMap>, S>,
    address_spaces: &'a HashMap<String, Rc<MemoryMap>>,
) -> Result<&'a Rc<MemoryMap>, SimError> {
    if let Some(name) = &pe.address_space {
        address_spaces
            .get(name)
            .ok_or_else(|| SimError(format!("Unknown address space '{name}'")))
    } else {
        memory_maps
            .get(&pe.memory_map)
            .ok_or_else(|| SimError(format!("Unknown memory map '{}'", pe.memory_map)))
    }
}

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

pub(crate) fn resolve(
    platform: &PlatformConfig,
    effective: &EffectiveConfigs,
    device_ids: &DeviceIds,
) -> Result<AddressMaps, SimError> {
    let physical: HashMap<_, _> = platform
        .memories
        .iter()
        .flatten()
        .zip(&effective.memories)
        .map(|(m, c)| {
            Ok((
                m.name.as_str(),
                MemoryRegion {
                    start: m.base_address,
                    end: *c.address_range()?.end(),
                    device: device_ids[&m.name],
                },
            ))
        })
        .collect::<Result<_, SimError>>()?;
    let graph = request_graph(platform)?;
    let mut memory_maps = HashMap::new();
    for map in &platform.memory_maps {
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
        memory_maps.insert(map.name.clone(), Rc::new(build_map(map, members)?));
        let Some(policy) = &map.address_mapping else {
            continue;
        };
        let stripe = policy.stripe_bytes;
        for (pe, config) in platform
            .processing_elements
            .iter()
            .flatten()
            .zip(&effective.processing_elements)
        {
            let compute = pe.address_space.is_none().then_some(pe.memory_map.as_str());
            let dma = pe.dma_memory_map.as_deref().or(compute);
            for (interface, selected) in [
                (pe.name.clone(), compute),
                (format!("{}.dma", pe.name), dma),
            ] {
                if selected != Some(map.name.as_str()) {
                    continue;
                }
                validate_fetch(&map.name, stripe, config.lsu_access_bytes, &interface)?;
                validate_cache_fetches(platform, effective, &graph, &map.name, stripe, &interface)?;
            }
        }
    }
    // Named stripes translate at the PE, so downstream caches fill lines in
    // a physical bank using its already validated alignment.
    let address_spaces = resolve_address_spaces(platform, &memory_maps, device_ids)?;
    Ok(AddressMaps {
        memory_maps,
        address_spaces,
        request_graph: graph,
    })
}

pub(crate) fn resolve_address_spaces<S: BuildHasher>(
    platform: &PlatformConfig,
    memory_maps: &HashMap<String, Rc<MemoryMap>, S>,
    device_ids: &DeviceIds,
) -> Result<HashMap<String, Rc<MemoryMap>>, SimError> {
    platform
        .address_spaces
        .iter()
        .map(|space| {
            let map = memory_maps.get(&space.memory_map).ok_or_else(|| {
                SimError(format!(
                    "Unknown memory map '{}' for address space '{}'",
                    space.memory_map, space.name
                ))
            })?;
            let resolved = build_address_space(space, map, device_ids)
                .map_err(|error| SimError(format!("Address space '{}': {error}", space.name)))?;
            Ok((space.name.clone(), Rc::new(resolved.into_memory_map())))
        })
        .collect()
}

fn build_address_space(
    section: &AddressSpaceSection,
    physical: &MemoryMap,
    device_ids: &DeviceIds,
) -> Result<AddressSpace, SimError> {
    let mut space = AddressSpace::new(section.access_size_bytes()? as u64)?;
    let mut claimed = HashSet::new();
    for region in section.address_regions.iter().flatten() {
        let AddressRegionSection::Striped { devices } = region;
        let mut banks = Vec::new();
        let mut bank_size = None;
        let mut previous_end: Option<u64> = None;
        for name in devices {
            let bank = device_ids
                .get(name)
                .and_then(|id| {
                    physical
                        .physical_regions()
                        .find(|region| region.device == *id)
                })
                .ok_or_else(|| {
                    SimError(format!(
                        "Memory '{name}' in striped region is not in memory map '{}'",
                        section.memory_map
                    ))
                })?;
            if !claimed.insert(bank.device) {
                return Err(SimError(format!(
                    "Duplicate memory '{name}' in address space"
                )));
            }
            let size = bank.end - bank.start + 1;
            if bank_size.is_some_and(|expected| size != expected) {
                return Err(SimError(
                    "Striped region has unequal bank capacities".into(),
                ));
            }
            if previous_end.is_some_and(|end| end.checked_add(1) != Some(bank.start)) {
                return Err(SimError(format!(
                    "Memory '{name}' in striped region is not contiguous with the previous bank in increasing physical address order"
                )));
            }
            bank_size = Some(size);
            previous_end = Some(bank.end);
            banks.push((bank.device, bank.start));
        }
        let device_size_bytes =
            bank_size.ok_or_else(|| SimError("Striped region has no banks".into()))?;
        let start = banks[0].1;
        let size = device_size_bytes
            .checked_mul(banks.len() as u64)
            .ok_or_else(|| {
                SimError(format!(
                    "Striped region at {start:#x} capacity overflows u64"
                ))
            })?;
        space.insert(
            start,
            size,
            AddressRegionPolicy::Striped {
                device_size_bytes,
                banks,
            },
        )?;
    }
    for region in physical.physical_regions() {
        if !claimed.contains(&region.device) {
            space.insert(
                region.start,
                region.end - region.start + 1,
                AddressRegionPolicy::Passthrough {
                    device: region.device,
                },
            )?;
        }
    }
    Ok(space)
}

fn validate_cache_fetches(
    platform: &PlatformConfig,
    effective: &EffectiveConfigs,
    graph: &HashMap<String, Vec<String>>,
    map: &str,
    stripe: u64,
    interface: &str,
) -> SimResult {
    let start = format!("pe.{interface}");
    let reachable = reachable_request_nodes(graph, &start);
    for (cache, cache_config) in platform.caches.iter().flatten().zip(&effective.caches) {
        if reachable.contains(format!("cache.{}.dev", cache.name).as_str()) {
            validate_fetch(map, stripe, cache_config.line_size_bytes(), &cache.name)?;
        }
    }
    Ok(())
}

pub(crate) fn reachable_request_nodes<'a>(
    graph: &'a HashMap<String, Vec<String>>,
    start: &'a str,
) -> HashSet<&'a str> {
    let mut pending = vec![start];
    let mut seen = HashSet::new();
    while let Some(node) = pending.pop() {
        if seen.insert(node) {
            pending.extend(graph.get(node).into_iter().flatten().map(String::as_str));
        }
    }
    seen
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
pub(crate) fn request_graph(
    platform: &PlatformConfig,
) -> Result<HashMap<String, Vec<String>>, SimError> {
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
                    let side = cache_connection_port(port.as_ref(), peer, left);
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
