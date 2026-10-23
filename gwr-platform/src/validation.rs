// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{HashMap, HashSet};

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};

use crate::builder::EffectiveConfigs;
use crate::connection_id::{CachePortId, ConnectionEndpointId, parse_connection_endpoint_id};
use crate::types::{CacheSection, FabricSection, MemoryMapSection, MemorySection, PlatformConfig};

impl PlatformConfig {
    /// Validate platform properties that do not require building the simulator.
    ///
    /// # Errors
    ///
    /// Returns an error if a fabric or physical memory range is invalid, or if
    /// a memory map or processing element references an unknown object, or if
    /// a physical memory is not aligned to a processing element's LSU access
    /// size or a cache's line size.
    pub fn validate(&self) -> SimResult {
        self.validated().map(drop)
    }

    pub(crate) fn validated(&self) -> Result<EffectiveConfigs, SimError> {
        let lookup = self.validation_lookup()?;
        let effective = EffectiveConfigs::new(self)?;
        self.validate_memory_ranges(&effective)?;
        self.validate_references(&lookup)?;
        self.validate_memory_fetch_alignment(&effective)?;
        self.validate_coherence(&effective)?;
        self.validate_connections(&lookup, &effective)?;
        Ok(effective)
    }

    fn validation_lookup(&self) -> Result<PlatformValidationLookup<'_>, SimError> {
        let mut devices = HashMap::new();
        for pe in self.processing_elements.iter().flatten() {
            insert_device(
                &mut devices,
                pe.name.as_str(),
                PlatformDevice::ProcessingElement,
            )?;
        }
        for memory in self.memories.iter().flatten() {
            insert_device(
                &mut devices,
                memory.name.as_str(),
                PlatformDevice::Memory(memory),
            )?;
        }

        for manager in self.coherency_managers.iter().flatten() {
            insert_device(
                &mut devices,
                &manager.name,
                PlatformDevice::CoherencyManager,
            )?;
        }
        let memory_maps =
            collect_unique_sections("memory map", self.memory_maps.iter(), |memory_map| {
                memory_map.name.as_str()
            })?;
        let caches = collect_unique_sections("cache", self.caches.iter().flatten(), |cache| {
            cache.name.as_str()
        })?;
        for cache in self.caches.iter().flatten() {
            insert_device(&mut devices, &cache.name, PlatformDevice::Cache)?;
        }
        let mut fabrics = HashMap::new();
        for (config_index, fabric) in self.fabrics.iter().flatten().enumerate() {
            if fabrics
                .insert(
                    fabric.name.as_str(),
                    FabricReference {
                        section: fabric,
                        config_index,
                    },
                )
                .is_some()
            {
                return sim_error!("Duplicate fabric name {}", fabric.name);
            }
        }

        Ok(PlatformValidationLookup {
            devices,
            memory_maps,
            caches,
            fabrics,
        })
    }

    fn validate_references(&self, lookup: &PlatformValidationLookup<'_>) -> SimResult {
        for memory_map in &self.memory_maps {
            let mut devices = HashSet::new();
            for device in &memory_map.devices {
                if lookup.memory(device.as_str()).is_none() {
                    return sim_error!(
                        "Unknown memory '{}' in memory map '{}'",
                        device,
                        memory_map.name
                    );
                }
                if !devices.insert(device.as_str()) {
                    return sim_error!(
                        "Duplicate memory '{}' in memory map '{}'",
                        device,
                        memory_map.name
                    );
                }
            }
        }

        for pe in self.processing_elements.iter().flatten() {
            if !lookup.memory_maps.contains_key(pe.memory_map.as_str()) {
                return sim_error!(
                    "Unknown memory map '{}' for processing element '{}'",
                    pe.memory_map,
                    pe.name
                );
            }
        }
        Ok(())
    }

    fn validate_connections(
        &self,
        lookup: &PlatformValidationLookup<'_>,
        effective: &EffectiveConfigs,
    ) -> SimResult {
        let mut connected_ports = HashSet::new();
        for connection in self.connections.iter().flatten() {
            let [from_source, to_source] = connection.connect.as_slice() else {
                return sim_error!(
                    "Invalid 'connect' {:?} with {} entries (only 2 expected)",
                    connection.connect,
                    connection.connect.len()
                );
            };

            validate_connection(
                lookup,
                effective,
                from_source,
                to_source,
                &mut connected_ports,
            )
            .map_err(|error| {
                SimError(format!(
                    "Connection '{from_source}' -> '{to_source}': {error}"
                ))
            })?;
        }
        Ok(())
    }

    /// Validate every fetch size, including non-power-of-two sizes, without
    /// computing an LCM that could overflow. Applies to all physical memories.
    fn validate_memory_fetch_alignment(&self, effective: &EffectiveConfigs) -> SimResult {
        let fetches =
            self.processing_elements
                .iter()
                .flatten()
                .zip(&effective.processing_elements)
                .map(|(pe, config)| {
                    (
                        format!(
                            "Processing element '{}', memory map '{}', LSU access size {}",
                            pe.name, pe.memory_map, config.lsu_access_bytes,
                        ),
                        config.lsu_access_bytes,
                    )
                })
                .chain(self.caches.iter().flatten().zip(&effective.caches).map(
                    |(cache, config)| (format!("cache '{}'", cache.name), config.line_size_bytes()),
                ));
        for (source, bytes) in fetches {
            let bytes = u64::try_from(bytes)
                .map_err(|error| SimError(format!("{source} fetch size: {error}")))?;
            for memory in self.memories.iter().flatten() {
                if !memory.base_address.is_multiple_of(bytes) {
                    return sim_error!(
                        "{source}: Memory '{}': base address {:#x} must be aligned to the {}-byte fetch size",
                        memory.name,
                        memory.base_address,
                        bytes
                    );
                }
                if !memory.config.capacity_bytes.is_multiple_of(bytes) {
                    return sim_error!(
                        "{source}: Memory '{}': capacity {} bytes must be a multiple of the {}-byte fetch size",
                        memory.name,
                        memory.config.capacity_bytes,
                        bytes
                    );
                }
            }
        }
        Ok(())
    }

    fn validate_memory_ranges(&self, effective: &EffectiveConfigs) -> SimResult {
        let mut ranges = self
            .memories
            .iter()
            .flatten()
            .zip(&effective.memories)
            .map(|(memory, config)| {
                let range = config
                    .address_range()
                    .map_err(|error| SimError(format!("Memory '{}': {error}", memory.name)))?;
                Ok((memory, *range.end()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        ranges.sort_by_key(|(memory, _)| memory.base_address);

        for pair in ranges.windows(2) {
            let (left, left_last_address) = &pair[0];
            let (right, right_last_address) = &pair[1];
            if right.base_address <= *left_last_address {
                return sim_error!(
                    "Physical memory ranges overlap: '{}' ({:#x}..={:#x}) and '{}' ({:#x}..={:#x})",
                    left.name,
                    left.base_address,
                    left_last_address,
                    right.name,
                    right.base_address,
                    right_last_address,
                );
            }
        }
        Ok(())
    }
}

struct FabricReference<'a> {
    section: &'a FabricSection,
    config_index: usize,
}

enum PlatformDevice<'a> {
    ProcessingElement,
    CoherencyManager,
    Cache,
    Memory(&'a MemorySection),
}

struct PlatformValidationLookup<'a> {
    devices: HashMap<&'a str, PlatformDevice<'a>>,
    memory_maps: HashMap<&'a str, &'a MemoryMapSection>,
    caches: HashMap<&'a str, &'a CacheSection>,
    fabrics: HashMap<&'a str, FabricReference<'a>>,
}

impl PlatformValidationLookup<'_> {
    fn validate_port_endpoint(
        &self,
        endpoint: &ConnectionEndpointId,
        source: &str,
        effective: &EffectiveConfigs,
    ) -> SimResult {
        match endpoint {
            ConnectionEndpointId::Null => Ok(()),
            ConnectionEndpointId::CoherencyManager { name } => {
                if !matches!(
                    self.devices.get(name.as_str()),
                    Some(PlatformDevice::CoherencyManager)
                ) {
                    return sim_error!("No Coherency Manager '{name}'");
                }
                Ok(())
            }
            ConnectionEndpointId::Pe { name } => {
                if !self.processing_element_exists(name) {
                    return sim_error!("No PE '{name}'");
                }
                Ok(())
            }
            ConnectionEndpointId::Cache { name, .. } => {
                if !self.caches.contains_key(name.as_str()) {
                    return sim_error!("No Cache '{name}'");
                }
                Ok(())
            }
            ConnectionEndpointId::Mem { name } => {
                if self.memory(name).is_none() {
                    return sim_error!("No Memory '{name}'");
                }
                Ok(())
            }
            ConnectionEndpointId::FabricPort {
                fabric,
                column,
                row,
                port,
            } => {
                let Some(fabric_ref) = self.fabrics.get(fabric.as_str()) else {
                    return sim_error!("No Fabric '{fabric}'");
                };
                validate_fabric_port(source, fabric_ref, effective, *column, *row, *port)
            }
        }
    }

    fn processing_element_exists(&self, name: &str) -> bool {
        matches!(
            self.devices.get(name),
            Some(PlatformDevice::ProcessingElement)
        )
    }

    fn memory(&self, name: &str) -> Option<&MemorySection> {
        match self.devices.get(name) {
            Some(PlatformDevice::Memory(memory)) => Some(memory),
            _ => None,
        }
    }
}

fn validate_connection(
    lookup: &PlatformValidationLookup<'_>,
    effective: &EffectiveConfigs,
    from_source: &str,
    to_source: &str,
    connected_ports: &mut HashSet<String>,
) -> SimResult {
    let from = parse_connection_endpoint_id(from_source)?;
    let to = parse_connection_endpoint_id(to_source)?;
    lookup.validate_port_endpoint(&from, from_source, effective)?;
    lookup.validate_port_endpoint(&to, to_source, effective)?;
    validate_port_endpoint_pair(&from, &to)?;
    validate_ports_available(&from, &to, connected_ports)
}

fn insert_device<'a>(
    devices: &mut HashMap<&'a str, PlatformDevice<'a>>,
    name: &'a str,
    device: PlatformDevice<'a>,
) -> SimResult {
    if devices.insert(name, device).is_some() {
        return sim_error!("Duplicate device name {name}");
    }
    Ok(())
}

fn collect_unique_sections<'a, T>(
    section: &str,
    sections: impl Iterator<Item = &'a T>,
    get_name: impl Fn(&'a T) -> &'a str,
) -> Result<HashMap<&'a str, &'a T>, SimError> {
    let mut sections_by_name = HashMap::new();
    for value in sections {
        let name = get_name(value);
        if sections_by_name.insert(name, value).is_some() {
            return sim_error!("Duplicate {section} name {name}");
        }
    }
    Ok(sections_by_name)
}

fn validate_ports_available(
    from: &ConnectionEndpointId,
    to: &ConnectionEndpointId,
    connected_ports: &mut HashSet<String>,
) -> SimResult {
    for port in connection_ports(from, to) {
        if !connected_ports.insert(port.clone()) {
            return sim_error!("Port '{port}' is connected more than once");
        }
    }
    Ok(())
}

fn connection_ports(from: &ConnectionEndpointId, to: &ConnectionEndpointId) -> Vec<String> {
    [
        (!matches!(from, ConnectionEndpointId::Null)).then(|| endpoint_port(from, to, true)),
        (!matches!(to, ConnectionEndpointId::Null)).then(|| endpoint_port(to, from, false)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn endpoint_port(
    endpoint: &ConnectionEndpointId,
    other: &ConnectionEndpointId,
    is_from: bool,
) -> String {
    match endpoint {
        ConnectionEndpointId::Null => unreachable!("null has no port to reserve"),
        ConnectionEndpointId::Pe { name } => format!("pe.{name}"),
        ConnectionEndpointId::CoherencyManager { name } => format!("coherency_manager.{name}"),
        ConnectionEndpointId::Mem { name } => format!("mem.{name}"),
        ConnectionEndpointId::FabricPort {
            fabric,
            column,
            row,
            port,
        } => format!("fabric.{fabric}@({column},{row}).{port}"),
        ConnectionEndpointId::Cache { name, port } => {
            let port = cache_connection_port(port.as_ref(), other, is_from);
            format!("cache.{name}.{port}")
        }
    }
}

fn cache_connection_port(
    port: Option<&CachePortId>,
    other: &ConnectionEndpointId,
    is_from: bool,
) -> &'static str {
    if let Some(port) = port {
        return port.as_str();
    }
    match other {
        ConnectionEndpointId::Null => "mem",
        ConnectionEndpointId::Pe { .. } => "dev",
        ConnectionEndpointId::Cache { .. } if is_from => "mem",
        ConnectionEndpointId::Cache { .. } => "dev",
        ConnectionEndpointId::Mem { .. }
        | ConnectionEndpointId::CoherencyManager { .. }
        | ConnectionEndpointId::FabricPort { .. } => "mem",
    }
}

fn validate_port_endpoint_pair(
    from: &ConnectionEndpointId,
    to: &ConnectionEndpointId,
) -> SimResult {
    match (from, to) {
        (
            ConnectionEndpointId::CoherencyManager { .. },
            ConnectionEndpointId::FabricPort { .. },
        )
        | (
            ConnectionEndpointId::FabricPort { .. },
            ConnectionEndpointId::CoherencyManager { .. },
        ) => Ok(()),
        (ConnectionEndpointId::CoherencyManager { .. }, _)
        | (_, ConnectionEndpointId::CoherencyManager { .. }) => {
            sim_error!("Coherency Managers must connect to a Fabric")
        }
        (ConnectionEndpointId::Null, ConnectionEndpointId::FabricPort { .. })
        | (ConnectionEndpointId::FabricPort { .. }, ConnectionEndpointId::Null)
        | (ConnectionEndpointId::Null, ConnectionEndpointId::Cache { .. })
        | (ConnectionEndpointId::Cache { .. }, ConnectionEndpointId::Null) => Ok(()),
        (ConnectionEndpointId::Null, _) | (_, ConnectionEndpointId::Null) => {
            sim_error!("Null can only connect to a Fabric or Cache port")
        }
        (ConnectionEndpointId::Pe { .. }, ConnectionEndpointId::Pe { .. }) => {
            sim_error!("Cannot connect a PE directly to a PE")
        }
        (ConnectionEndpointId::Mem { .. }, ConnectionEndpointId::Mem { .. }) => {
            sim_error!("Cannot connect a Memory directly to a Memory")
        }
        (ConnectionEndpointId::Pe { .. }, ConnectionEndpointId::Cache { port, .. })
        | (ConnectionEndpointId::Cache { port, .. }, ConnectionEndpointId::Pe { .. }) => {
            validate_cache_dev_port(port.as_ref())
        }
        (ConnectionEndpointId::Cache { port, .. }, ConnectionEndpointId::FabricPort { .. })
        | (ConnectionEndpointId::FabricPort { .. }, ConnectionEndpointId::Cache { port, .. }) => {
            validate_cache_mem_port(
                port.as_ref(),
                "Cache should connect the 'mem' port to a Fabric",
            )
        }
        (ConnectionEndpointId::Cache { port, .. }, ConnectionEndpointId::Mem { .. })
        | (ConnectionEndpointId::Mem { .. }, ConnectionEndpointId::Cache { port, .. }) => {
            validate_cache_mem_port(
                port.as_ref(),
                "Cache should connect the 'mem' port to a Memory",
            )
        }
        (
            ConnectionEndpointId::Cache {
                port: from_port, ..
            },
            ConnectionEndpointId::Cache { port: to_port, .. },
        ) => {
            if from_port
                .as_ref()
                .is_some_and(|port| port != &CachePortId::Mem)
                || to_port
                    .as_ref()
                    .is_some_and(|port| port != &CachePortId::Dev)
            {
                return sim_error!(
                    "When connecting Cache to Cache, connect 'mem' to 'dev' (or simply don't specify ports)"
                );
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_cache_dev_port(port: Option<&CachePortId>) -> SimResult {
    if port.is_some_and(|port| port != &CachePortId::Dev) {
        return sim_error!("PEs can only connect to the 'dev' port on the Cache");
    }
    Ok(())
}

fn validate_cache_mem_port(port: Option<&CachePortId>, message: &str) -> SimResult {
    if port.is_some_and(|port| port != &CachePortId::Mem) {
        return sim_error!("{message}");
    }
    Ok(())
}

fn validate_fabric_port(
    source: &str,
    fabric: &FabricReference<'_>,
    effective: &EffectiveConfigs,
    col: usize,
    row: usize,
    port: usize,
) -> SimResult {
    let section = fabric.section;
    let config = &effective.fabrics[fabric.config_index].0;
    let ports_per_node = config.num_ports_per_node();
    if col >= config.num_columns() || row >= config.num_rows() || port >= ports_per_node {
        return sim_error!(
            "Fabric port '{source}' is out of range for fabric '{}' with {} columns, {} rows and {} ports per node",
            section.name,
            config.num_columns(),
            config.num_rows(),
            ports_per_node,
        );
    }
    Ok(())
}

impl PlatformConfig {
    fn validate_coherence(&self, effective: &EffectiveConfigs) -> SimResult {
        let managers = self
            .coherency_managers
            .iter()
            .flatten()
            .map(|m| (m.name.as_str(), m))
            .collect::<HashMap<_, _>>();
        let maps = self
            .memory_maps
            .iter()
            .map(|m| (m.name.as_str(), m))
            .collect::<HashMap<_, _>>();
        self.validate_manager_domains(&managers, &maps)?;
        self.validate_cache_coherence(effective, &managers, &maps)
    }

    fn validate_manager_domains(
        &self,
        managers: &HashMap<&str, &crate::types::CoherencyManagerSection>,
        maps: &HashMap<&str, &MemoryMapSection>,
    ) -> SimResult {
        let memories = self
            .memories
            .iter()
            .flatten()
            .map(|m| (m.name.as_str(), m))
            .collect::<HashMap<_, _>>();
        let mut memory_owners = HashMap::new();
        for manager in managers.values() {
            let line = manager
                .config
                .line_size_bytes
                .unwrap_or(crate::builder::DEFAULT_CACHE_LINE_SIZE_BYTES);
            if line == 0 {
                return sim_error!(
                    "Coherency Manager '{}': line size must be positive",
                    manager.name
                );
            }
            let map = maps.get(manager.memory_map.as_str()).ok_or_else(|| {
                SimError(format!(
                    "Unknown memory map '{}' for Coherency Manager '{}'",
                    manager.memory_map, manager.name
                ))
            })?;
            if map.devices.is_empty() {
                return sim_error!(
                    "Coherency Manager '{}' has an empty memory map",
                    manager.name
                );
            }
            for name in &map.devices {
                if let Some(previous) = memory_owners.insert(name.as_str(), manager.name.as_str()) {
                    return sim_error!(
                        "Memory '{}' is assigned to multiple coherency managers '{}' and '{}'",
                        name,
                        previous,
                        manager.name
                    );
                }
                let memory = memories[name.as_str()];
                if !(memory.base_address as u128).is_multiple_of(line as u128)
                    || !(memory.config.capacity_bytes as u128).is_multiple_of(line as u128)
                {
                    return sim_error!(
                        "Coherency Manager '{}': Memory '{}' must align to its {}-byte line size",
                        manager.name,
                        name,
                        line
                    );
                }
            }
        }
        Ok(())
    }

    fn validate_cache_coherence(
        &self,
        effective: &EffectiveConfigs,
        managers: &HashMap<&str, &crate::types::CoherencyManagerSection>,
        maps: &HashMap<&str, &MemoryMapSection>,
    ) -> SimResult {
        for (cache, config) in self.caches.iter().flatten().zip(&effective.caches) {
            if let Some(names) = &cache.coherency_managers {
                if names.is_empty() {
                    return sim_error!(
                        "Cache '{}' has an empty coherency manager list",
                        cache.name
                    );
                }
                let mut covered = HashSet::new();
                for name in names {
                    let manager = managers.get(name.as_str()).ok_or_else(|| {
                        SimError(format!(
                            "Unknown coherency manager '{name}' for Cache '{}'",
                            cache.name
                        ))
                    })?;
                    let manager_line_size_bytes = manager
                        .config
                        .line_size_bytes
                        .unwrap_or(crate::builder::DEFAULT_CACHE_LINE_SIZE_BYTES);
                    if manager_line_size_bytes != config.line_size_bytes() {
                        return sim_error!(
                            "Cache '{}' line size {} does not match Coherency Manager '{}' line size {}",
                            cache.name,
                            config.line_size_bytes(),
                            name,
                            manager_line_size_bytes
                        );
                    }
                    for memory in &maps[manager.memory_map.as_str()].devices {
                        if !covered.insert(memory.as_str()) {
                            return sim_error!(
                                "Cache '{}' maps memory '{}' to multiple coherency managers",
                                cache.name,
                                memory
                            );
                        }
                    }
                }
                for connection in self.connections.iter().flatten() {
                    let [left, right] = connection.connect.as_slice() else {
                        continue;
                    };
                    let left = parse_connection_endpoint_id(left)?;
                    let right = parse_connection_endpoint_id(right)?;
                    let peer = match (&left, &right) {
                        (ConnectionEndpointId::Cache { name, .. }, peer) if name == &cache.name => {
                            peer
                        }
                        (peer, ConnectionEndpointId::Cache { name, .. }) if name == &cache.name => {
                            peer
                        }
                        _ => continue,
                    };
                    match peer {
                        ConnectionEndpointId::Cache { .. } => {
                            return sim_error!(
                                "Cache '{}': hierarchical coherence is unsupported",
                                cache.name
                            );
                        }
                        ConnectionEndpointId::Mem { .. } => {
                            return sim_error!(
                                "Coherent Cache '{}' must connect through a Fabric to its managers",
                                cache.name
                            );
                        }
                        ConnectionEndpointId::Pe { name } => {
                            let pe = self
                                .processing_elements
                                .iter()
                                .flatten()
                                .find(|pe| &pe.name == name)
                                .ok_or_else(|| SimError(format!("No PE '{name}'")))?;
                            for memory in &maps[pe.memory_map.as_str()].devices {
                                if !covered.contains(memory.as_str()) {
                                    return sim_error!(
                                        "Cache '{}' has no coherency manager for Memory '{}' used by PE '{}'",
                                        cache.name,
                                        memory,
                                        name
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
}
