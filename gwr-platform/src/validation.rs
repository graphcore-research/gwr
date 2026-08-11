// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{HashMap, HashSet};

use gwr_engine::sim_error;
use gwr_engine::types::SimResult;

use crate::builder::DEFAULT_FABRIC_PORTS_PER_NODE;
use crate::connect::{PortEndpoint, parse_port_endpoint, validate_port_endpoint_pair};
use crate::types::{FabricSection, MemorySection, PlatformConfig};

impl PlatformConfig {
    /// Validate platform properties that do not require building the simulator.
    ///
    /// # Errors
    ///
    /// Returns an error if a physical memory range is invalid or if a memory
    /// map or processing element references an unknown object.
    pub fn validate(&self) -> SimResult {
        validate_unique_names(self)?;
        validate_memory_ranges(self.memories.as_deref().unwrap_or_default())?;
        validate_references(self)?;
        validate_connections(self)
    }
}

fn validate_unique_names(platform: &PlatformConfig) -> SimResult {
    let mut device_names = HashSet::new();
    for pe in platform.processing_elements.iter().flatten() {
        if !device_names.insert(pe.name.as_str()) {
            return sim_error!("Duplicate device name {}", pe.name);
        }
    }
    for memory in platform.memories.iter().flatten() {
        if !device_names.insert(memory.name.as_str()) {
            return sim_error!("Duplicate device name {}", memory.name);
        }
    }

    validate_unique_section_names(
        "memory map",
        platform.memory_maps.iter().map(|map| &map.name),
    )?;
    validate_unique_section_names(
        "cache",
        platform.caches.iter().flatten().map(|cache| &cache.name),
    )?;
    validate_unique_section_names(
        "fabric",
        platform.fabrics.iter().flatten().map(|fabric| &fabric.name),
    )
}

fn validate_unique_section_names<'a>(
    section: &str,
    names: impl Iterator<Item = &'a String>,
) -> SimResult {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name.as_str()) {
            return sim_error!("Duplicate {section} name {name}");
        }
    }
    Ok(())
}

fn validate_references(platform: &PlatformConfig) -> SimResult {
    let memories = platform
        .memories
        .iter()
        .flatten()
        .map(|memory| (memory.name.as_str(), memory))
        .collect::<HashMap<_, _>>();
    for memory_map in &platform.memory_maps {
        let mut devices = HashSet::new();
        for device in &memory_map.devices {
            let Some(memory) = memories.get(device.name.as_str()).copied() else {
                return sim_error!(
                    "Unknown memory '{}' in memory map '{}'",
                    device.name,
                    memory_map.name
                );
            };
            if memory.capacity_bytes == 0 {
                return sim_error!(
                    "Memory '{}' in memory map '{}' has zero capacity",
                    memory.name,
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

    let memory_map_names = platform
        .memory_maps
        .iter()
        .map(|memory_map| memory_map.name.as_str())
        .collect::<HashSet<_>>();
    for pe in platform.processing_elements.iter().flatten() {
        if !memory_map_names.contains(pe.memory_map.as_str()) {
            return sim_error!(
                "Unknown memory map '{}' for processing element '{}'",
                pe.memory_map,
                pe.name
            );
        }
    }
    Ok(())
}

fn validate_connections(platform: &PlatformConfig) -> SimResult {
    let pe_names = platform
        .processing_elements
        .iter()
        .flatten()
        .map(|pe| pe.name.as_str())
        .collect::<HashSet<_>>();
    let cache_names = platform
        .caches
        .iter()
        .flatten()
        .map(|cache| cache.name.as_str())
        .collect::<HashSet<_>>();
    let memory_names = platform
        .memories
        .iter()
        .flatten()
        .map(|memory| memory.name.as_str())
        .collect::<HashSet<_>>();
    let fabrics = platform
        .fabrics
        .iter()
        .flatten()
        .map(|fabric| (fabric.name.as_str(), fabric))
        .collect::<HashMap<_, _>>();

    let mut connected_ports = HashSet::new();
    for connection in platform.connections.iter().flatten() {
        if connection.connect.len() != 2 {
            return sim_error!(
                "Invalid 'connect' with {} entries (only 2 expected)",
                connection.connect.len()
            );
        }
        let from = parse_port_endpoint(&connection.connect[0])?;
        let to = parse_port_endpoint(&connection.connect[1])?;
        validate_port_endpoint(
            &from,
            &pe_names,
            &cache_names,
            &memory_names,
            &fabrics,
            &connection.connect[0],
        )?;
        validate_port_endpoint(
            &to,
            &pe_names,
            &cache_names,
            &memory_names,
            &fabrics,
            &connection.connect[1],
        )?;
        validate_port_endpoint_pair(&from, &to)?;
        validate_ports_available(&from, &to, &mut connected_ports)?;
    }
    Ok(())
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

fn validate_port_endpoint(
    endpoint: &PortEndpoint<'_>,
    pe_names: &HashSet<&str>,
    cache_names: &HashSet<&str>,
    memory_names: &HashSet<&str>,
    fabrics: &HashMap<&str, &FabricSection>,
    source: &str,
) -> SimResult {
    match endpoint {
        PortEndpoint::Pe { name } => {
            if !pe_names.contains(name) {
                return sim_error!("No PE '{name}'");
            }
            Ok(())
        }
        PortEndpoint::Cache { name, .. } => {
            if !cache_names.contains(name) {
                return sim_error!("No Cache '{name}'");
            }
            Ok(())
        }
        PortEndpoint::Mem { name } => {
            if !memory_names.contains(name) {
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
            let Some(fabric) = fabrics.get(name).copied() else {
                return sim_error!("No Fabric '{name}'");
            };
            validate_fabric_port(source, fabric, *col, *row, *port)
        }
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

fn validate_memory_ranges(memories: &[MemorySection]) -> SimResult {
    let mut ranges = memories
        .iter()
        .filter(|memory| memory.capacity_bytes != 0)
        .map(|memory| {
            memory
                .base_address
                .checked_add(memory.capacity_bytes)
                .map(|end| (memory, end))
                .ok_or_else(|| {
                    gwr_engine::types::SimError(format!(
                        "Memory '{}' range overflows the physical address space",
                        memory.name
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    ranges.sort_by_key(|(memory, _)| memory.base_address);

    for pair in ranges.windows(2) {
        let [(left, left_end), (right, right_end)] = pair else {
            unreachable!();
        };
        if right.base_address < *left_end {
            return sim_error!(
                "Physical memory ranges overlap: '{}' ({:#x}..{:#x}) and '{}' ({:#x}..{:#x})",
                left.name,
                left.base_address,
                left_end,
                right.name,
                right.base_address,
                right_end,
            );
        }
    }
    Ok(())
}
