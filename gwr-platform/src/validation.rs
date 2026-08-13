// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{HashMap, HashSet};

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::memory::checked_last_address;

use crate::builder::DEFAULT_FABRIC_PORTS_PER_NODE;
use crate::connect::{PortEndpoint, parse_port_endpoint, validate_port_endpoint_pair};
use crate::types::{CacheSection, FabricSection, MemoryMapSection, MemorySection, PlatformConfig};

enum PlatformDevice<'a> {
    ProcessingElement,
    Memory(&'a MemorySection),
}

struct PlatformValidationLookup<'a> {
    devices: HashMap<&'a str, PlatformDevice<'a>>,
    memory_maps: HashMap<&'a str, &'a MemoryMapSection>,
    caches: HashMap<&'a str, &'a CacheSection>,
    fabrics: HashMap<&'a str, &'a FabricSection>,
}

impl PlatformValidationLookup<'_> {
    fn processing_element_exists(&self, name: &str) -> bool {
        matches!(
            self.devices.get(name),
            Some(PlatformDevice::ProcessingElement)
        )
    }

    fn memory(&self, name: &str) -> Option<&MemorySection> {
        match self.devices.get(name) {
            Some(PlatformDevice::Memory(memory)) => Some(memory),
            Some(PlatformDevice::ProcessingElement) | None => None,
        }
    }

    fn validate_port_endpoint(&self, endpoint: &PortEndpoint<'_>, source: &str) -> SimResult {
        match endpoint {
            PortEndpoint::Pe { name } => {
                if !self.processing_element_exists(name) {
                    return sim_error!("No PE '{name}'");
                }
                Ok(())
            }
            PortEndpoint::Cache { name, .. } => {
                if !self.caches.contains_key(name) {
                    return sim_error!("No Cache '{name}'");
                }
                Ok(())
            }
            PortEndpoint::Mem { name } => {
                if self.memory(name).is_none() {
                    return sim_error!("No Memory '{name}'");
                }
                Ok(())
            }
            PortEndpoint::FabricTile {
                name,
                col,
                row,
                port,
            } => {
                let Some(fabric) = self.fabrics.get(name).copied() else {
                    return sim_error!("No Fabric '{name}'");
                };
                validate_fabric_port(source, fabric, *col, *row, *port)
            }
        }
    }
}

impl PlatformConfig {
    /// Validate platform properties that do not require building the simulator.
    ///
    /// # Errors
    ///
    /// Returns an error if a fabric or physical memory range is invalid, or if
    /// a memory map or processing element references an unknown object.
    pub fn validate(&self) -> SimResult {
        let lookup = self.validation_lookup()?;
        self.validate_fabrics()?;
        self.validate_memory_ranges()?;
        self.validate_references(&lookup)?;
        self.validate_connections(&lookup)
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

        let memory_maps =
            collect_unique_sections("memory map", self.memory_maps.iter(), |memory_map| {
                memory_map.name.as_str()
            })?;
        let caches = collect_unique_sections("cache", self.caches.iter().flatten(), |cache| {
            cache.name.as_str()
        })?;
        let fabrics = collect_unique_sections("fabric", self.fabrics.iter().flatten(), |fabric| {
            fabric.name.as_str()
        })?;

        Ok(PlatformValidationLookup {
            devices,
            memory_maps,
            caches,
            fabrics,
        })
    }

    fn validate_fabrics(&self) -> SimResult {
        for fabric in self.fabrics.iter().flatten() {
            let ports_per_node = fabric
                .fabric_ports_per_node
                .unwrap_or(DEFAULT_FABRIC_PORTS_PER_NODE);
            let num_ports = fabric
                .columns
                .checked_mul(fabric.rows)
                .and_then(|nodes| nodes.checked_mul(ports_per_node))
                .ok_or_else(|| {
                    SimError(format!("Fabric '{}' port count overflows", fabric.name))
                })?;
            if num_ports < 2 {
                return sim_error!(
                    "Fabric '{}' has {num_ports} total ports; at least 2 are required",
                    fabric.name
                );
            }
        }
        Ok(())
    }

    fn validate_references(&self, lookup: &PlatformValidationLookup<'_>) -> SimResult {
        for memory_map in &self.memory_maps {
            let mut devices = HashSet::new();
            for device in &memory_map.devices {
                if lookup.memory(device.name.as_str()).is_none() {
                    return sim_error!(
                        "Unknown memory '{}' in memory map '{}'",
                        device.name,
                        memory_map.name
                    );
                }
                if !devices.insert(device.name.as_str()) {
                    return sim_error!(
                        "Duplicate memory '{}' in memory map '{}'",
                        device.name,
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

    fn validate_connections(&self, lookup: &PlatformValidationLookup<'_>) -> SimResult {
        let mut connected_ports = HashSet::new();
        for connection in self.connections.iter().flatten() {
            let [from_source, to_source] = connection.connect.as_slice() else {
                return sim_error!(
                    "Invalid 'connect' {:?} with {} entries (only 2 expected)",
                    connection.connect,
                    connection.connect.len()
                );
            };

            validate_connection(lookup, from_source, to_source, &mut connected_ports).map_err(
                |error| {
                    SimError(format!(
                        "Connection '{from_source}' -> '{to_source}': {error}"
                    ))
                },
            )?;
        }
        Ok(())
    }

    fn validate_memory_ranges(&self) -> SimResult {
        let mut ranges = self
            .memories
            .iter()
            .flatten()
            .map(|memory| {
                if memory.capacity_bytes == 0 {
                    return sim_error!("Memory '{}' has zero capacity", memory.name);
                }
                checked_last_address(memory.base_address, memory.capacity_bytes)
                    .map(|last_address| (memory, last_address))
                    .ok_or_else(|| {
                        SimError(format!(
                            "Memory '{}' range overflows the physical address space",
                            memory.name
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        ranges.sort_by_key(|(memory, _)| memory.base_address);

        for pair in ranges.windows(2) {
            let [(left, left_last_address), (right, right_last_address)] = pair else {
                unreachable!();
            };
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

fn validate_connection(
    lookup: &PlatformValidationLookup<'_>,
    from_source: &str,
    to_source: &str,
    connected_ports: &mut HashSet<String>,
) -> SimResult {
    let from = parse_port_endpoint(from_source)?;
    let to = parse_port_endpoint(to_source)?;
    lookup.validate_port_endpoint(&from, from_source)?;
    lookup.validate_port_endpoint(&to, to_source)?;
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
    from: &PortEndpoint<'_>,
    to: &PortEndpoint<'_>,
    connected_ports: &mut HashSet<String>,
) -> SimResult {
    for port in connection_ports(from, to) {
        if !connected_ports.insert(port.clone()) {
            return sim_error!("Port '{port}' is connected more than once");
        }
    }
    Ok(())
}

fn connection_ports(from: &PortEndpoint<'_>, to: &PortEndpoint<'_>) -> Vec<String> {
    vec![
        endpoint_port(from, to, true),
        endpoint_port(to, from, false),
    ]
}

fn endpoint_port(endpoint: &PortEndpoint<'_>, other: &PortEndpoint<'_>, is_from: bool) -> String {
    match endpoint {
        PortEndpoint::Pe { name } => format!("pe.{name}"),
        PortEndpoint::Mem { name } => format!("mem.{name}"),
        PortEndpoint::FabricTile {
            name,
            col,
            row,
            port,
        } => format!("fabric.{name}@({col},{row}).{port}"),
        PortEndpoint::Cache { name, port } => {
            let port = cache_connection_port(*port, other, is_from);
            format!("cache.{name}.{port}")
        }
    }
}

fn cache_connection_port(
    port: Option<&str>,
    other: &PortEndpoint<'_>,
    is_from: bool,
) -> &'static str {
    if let Some("dev") = port {
        return "dev";
    }
    if let Some("mem") = port {
        return "mem";
    }
    match other {
        PortEndpoint::Pe { .. } => "dev",
        PortEndpoint::Cache { .. } if is_from => "mem",
        PortEndpoint::Cache { .. } => "dev",
        PortEndpoint::Mem { .. } | PortEndpoint::FabricTile { .. } => "mem",
    }
}

fn validate_fabric_port(
    source: &str,
    fabric: &FabricSection,
    col: usize,
    row: usize,
    port: usize,
) -> SimResult {
    let ports_per_node = fabric
        .fabric_ports_per_node
        .unwrap_or(DEFAULT_FABRIC_PORTS_PER_NODE);
    if col >= fabric.columns || row >= fabric.rows || port >= ports_per_node {
        return sim_error!(
            "Fabric port '{source}' is out of range for fabric '{}' with {} columns, {} rows and {} ports per node",
            fabric.name,
            fabric.columns,
            fabric.rows,
            ports_per_node,
        );
    }
    Ok(())
}
