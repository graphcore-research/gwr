// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;
use std::sync::LazyLock;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::fabric::Fabric;
use gwr_models::memory::Memory;
use gwr_models::memory::cache::Cache;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::processing_element::ProcessingElement;
use gwr_track::debug;
use gwr_track::entity::GetEntity;
use regex::Regex;

use crate::Platform;
use crate::types::PlatformConfig;

pub enum PortId<'a> {
    Pe {
        pe: &'a Rc<ProcessingElement>,
    },
    Cache {
        cache: &'a Rc<Cache<MemoryAccess>>,
        port: Option<&'a str>,
    },
    Mem {
        memory: &'a Rc<Memory<MemoryAccess>>,
    },
    FabricTile {
        fabric: &'a Rc<dyn Fabric<MemoryAccess>>,
        port_idx: usize,
    },
}

#[derive(Debug)]
pub(crate) enum PortEndpoint<'a> {
    Pe {
        name: &'a str,
    },
    Cache {
        name: &'a str,
        port: Option<&'a str>,
    },
    Mem {
        name: &'a str,
    },
    FabricTile {
        name: &'a str,
        col: usize,
        row: usize,
        port: usize,
    },
}

pub fn connect_ports(platform: &Platform, cfg: &PlatformConfig) -> SimResult {
    if let Some(connections) = &cfg.connections {
        for c in connections {
            if c.connect.len() != 2 {
                return sim_error!(
                    "Invalid 'connect' with {} entries (only 2 expected)",
                    c.connect.len()
                );
            }

            let from_endpoint = parse_port_endpoint(&c.connect[0])?;
            let to_endpoint = parse_port_endpoint(&c.connect[1])?;
            validate_port_endpoint_pair(&from_endpoint, &to_endpoint)?;
            let from = resolve_port_endpoint(platform, &from_endpoint)?;
            let to = resolve_port_endpoint(platform, &to_endpoint)?;
            connect_port(platform, &from, &to)?;
        }
    }
    Ok(())
}

pub(crate) fn parse_port_endpoint(s: &str) -> Result<PortEndpoint<'_>, SimError> {
    let mut parts = s.split('.');
    let kind = parts
        .next()
        .ok_or_else(|| SimError(format!("Failed to parse kind in '{s}'")))?;

    if kind == "fabric" {
        return parse_fabric_endpoint(s);
    }

    // Parse ports IDs of the form: kind.name[.port]
    let name = parts
        .next()
        .ok_or_else(|| SimError(format!("Failed to parse name in '{s}'")))?;
    let port = parts.next();
    if parts.next().is_some() {
        return sim_error!("Failed to parse '{s}' - extra tokens");
    }

    match kind {
        "pe" => {
            if port.is_some() {
                return sim_error!("Cannot specify a port for PE");
            }
            Ok(PortEndpoint::Pe { name })
        }
        "cache" => Ok(PortEndpoint::Cache { name, port }),
        "mem" => {
            if port.is_some() {
                return sim_error!("Cannot specify a port for Memory");
            }
            Ok(PortEndpoint::Mem { name })
        }
        _ => sim_error!("Failed to parse '{s}' - unsupported kind"),
    }
}

pub(crate) fn validate_port_endpoint_pair(
    from: &PortEndpoint<'_>,
    to: &PortEndpoint<'_>,
) -> SimResult {
    match (from, to) {
        (PortEndpoint::Pe { .. }, PortEndpoint::Pe { .. }) => {
            sim_error!("Cannot connect a PE directly to a PE")
        }
        (PortEndpoint::Mem { .. }, PortEndpoint::Mem { .. }) => {
            sim_error!("Cannot connect a Memory directly to a Memory")
        }
        (PortEndpoint::Pe { .. }, PortEndpoint::Cache { port, .. })
        | (PortEndpoint::Cache { port, .. }, PortEndpoint::Pe { .. }) => {
            validate_cache_dev_port(*port)
        }
        (PortEndpoint::Cache { port, .. }, PortEndpoint::FabricTile { .. })
        | (PortEndpoint::FabricTile { .. }, PortEndpoint::Cache { port, .. }) => {
            validate_cache_mem_port(*port, "Cache should connect the 'mem' port to a Fabric")
        }
        (PortEndpoint::Cache { port, .. }, PortEndpoint::Mem { .. })
        | (PortEndpoint::Mem { .. }, PortEndpoint::Cache { port, .. }) => {
            validate_cache_mem_port(*port, "Cache should connect the 'mem' port to a Memory")
        }
        (
            PortEndpoint::Cache {
                port: from_port, ..
            },
            PortEndpoint::Cache { port: to_port, .. },
        ) => {
            if from_port.is_some_and(|port| port != "mem")
                || to_port.is_some_and(|port| port != "dev")
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

/// Parse a Fabric port ID of the form:
///   fabric.name@(col,row)[.port]
fn parse_fabric_endpoint(s: &str) -> Result<PortEndpoint<'_>, SimError> {
    static FABRIC_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^fabric\.([A-Za-z0-9_]+)@\((\d+),(\d+)\)(?:\.(.*))?$").unwrap()
    });

    if let Some(caps) = FABRIC_RE.captures(s) {
        let name = caps.get(1).unwrap().as_str();
        let col = caps[2].parse().map_err(|e| SimError(format!("{e}")))?;
        let row = caps[3].parse().map_err(|e| SimError(format!("{e}")))?;
        let port = caps
            .get(4)
            .map_or(Ok(0), |port| port.as_str().parse())
            .map_err(|e| SimError(format!("{e}")))?;

        Ok(PortEndpoint::FabricTile {
            name,
            col,
            row,
            port,
        })
    } else {
        sim_error!("Unable to parse Fabric port '{s}'")
    }
}

fn resolve_port_endpoint<'a>(
    platform: &'a Platform,
    endpoint: &PortEndpoint<'a>,
) -> Result<PortId<'a>, SimError> {
    match endpoint {
        PortEndpoint::Pe { name } => Ok(PortId::Pe {
            pe: platform.pe(name)?,
        }),
        PortEndpoint::Cache { name, port } => Ok(PortId::Cache {
            cache: platform.cache(name)?,
            port: *port,
        }),
        PortEndpoint::Mem { name } => Ok(PortId::Mem {
            memory: platform.memory(name)?,
        }),
        PortEndpoint::FabricTile {
            name,
            col,
            row,
            port,
        } => {
            let fabric = platform.fabric(name)?;
            let port_idx = fabric.col_row_port_to_fabric_port_index(*col, *row, *port);
            Ok(PortId::FabricTile { fabric, port_idx })
        }
    }
}

fn connect_port(platform: &Platform, from: &PortId, to: &PortId) -> SimResult {
    match from {
        PortId::Pe { pe } => connect_pe_to(platform, pe, to),
        PortId::Cache { cache, port } => connect_cache_to(platform, cache, *port, to),
        PortId::FabricTile { fabric, port_idx } => {
            connect_fabric_to(platform, fabric, *port_idx, to)
        }
        PortId::Mem { memory } => connect_memory_to(platform, memory, to),
    }
}

fn connect_pe_to(platform: &Platform, pe: &Rc<ProcessingElement>, to: &PortId) -> SimResult {
    match to {
        PortId::Pe { .. } => {
            sim_error!("Cannot connect a PE directly to a PE")
        }
        PortId::Cache { cache, port } => connect_pe_to_cache(platform, pe, cache, *port),
        PortId::FabricTile { fabric, port_idx } => {
            connect_pe_to_fabric(platform, pe, fabric, *port_idx)
        }
        PortId::Mem { memory } => connect_pe_to_memory(platform, pe, memory),
    }
}

fn connect_cache_to(
    platform: &Platform,
    cache: &Rc<Cache<MemoryAccess>>,
    cache_port: Option<&str>,
    to: &PortId,
) -> SimResult {
    match to {
        PortId::Pe { pe } => connect_pe_to_cache(platform, pe, cache, cache_port),
        PortId::Cache {
            cache: to_cache,
            port,
        } => connect_cache_to_cache(platform, cache, cache_port, to_cache, *port),
        PortId::FabricTile { fabric, port_idx } => {
            connect_cache_to_fabric(platform, cache, cache_port, fabric, *port_idx)
        }
        PortId::Mem { memory } => connect_cache_to_memory(platform, cache, cache_port, memory),
    }
}

fn connect_fabric_to(
    platform: &Platform,
    fabric: &Rc<dyn Fabric<MemoryAccess>>,
    fabric_port_idx: usize,
    to: &PortId,
) -> SimResult {
    match to {
        PortId::Pe { pe } => connect_pe_to_fabric(platform, pe, fabric, fabric_port_idx),
        PortId::Cache { cache, port } => {
            connect_cache_to_fabric(platform, cache, *port, fabric, fabric_port_idx)
        }
        PortId::FabricTile {
            fabric: to_fabric,
            port_idx: to_port_idx,
        } => connect_fabric_to_fabric(platform, fabric, fabric_port_idx, to_fabric, *to_port_idx),
        PortId::Mem { memory } => {
            connect_memory_to_fabric(platform, memory, fabric, fabric_port_idx)
        }
    }
}

fn connect_memory_to(
    platform: &Platform,
    memory: &Rc<Memory<MemoryAccess>>,
    to: &PortId,
) -> SimResult {
    match to {
        PortId::Pe { pe } => connect_pe_to_memory(platform, pe, memory),
        PortId::Cache { cache, port } => connect_cache_to_memory(platform, cache, *port, memory),
        PortId::FabricTile { fabric, port_idx } => {
            connect_memory_to_fabric(platform, memory, fabric, *port_idx)
        }
        PortId::Mem { .. } => {
            sim_error!("Cannot connect a Memory directly to a Memory")
        }
    }
}

fn connect_pe_to_cache(
    platform: &Platform,
    pe: &Rc<ProcessingElement>,
    cache: &Rc<Cache<MemoryAccess>>,
    cache_port: Option<&str>,
) -> SimResult {
    validate_cache_dev_port(cache_port)?;

    debug!(platform.entity() ; "Connect {} to {}.dev", pe, cache);
    pe.connect_port_tx(cache.port_dev_rx())?;
    cache.connect_port_dev_tx(pe.port_rx())
}

fn connect_pe_to_fabric(
    platform: &Platform,
    pe: &Rc<ProcessingElement>,
    fabric: &Rc<dyn Fabric<MemoryAccess>>,
    fabric_port_idx: usize,
) -> SimResult {
    debug!(platform.entity() ; "Connect {} to {}.{}", pe, fabric, fabric_port_idx);
    pe.connect_port_tx(fabric.port_ingress_i(fabric_port_idx))?;
    fabric.connect_port_egress_i(fabric_port_idx, pe.port_rx())
}

fn connect_pe_to_memory(
    platform: &Platform,
    pe: &Rc<ProcessingElement>,
    mem: &Rc<Memory<MemoryAccess>>,
) -> SimResult {
    debug!(platform.entity() ; "Connect {} to {}.dev", pe, mem);
    pe.connect_port_tx(mem.port_rx())?;
    mem.connect_port_tx(pe.port_rx())
}

fn connect_cache_to_fabric(
    platform: &Platform,
    cache: &Rc<Cache<MemoryAccess>>,
    cache_port: Option<&str>,
    fabric: &Rc<dyn Fabric<MemoryAccess>>,
    fabric_port_idx: usize,
) -> SimResult {
    validate_cache_mem_port(
        cache_port,
        "Cache should connect the 'mem' port to a Fabric",
    )?;

    debug!(platform.entity() ; "Connect {}.mem to {}.{}", cache, fabric, fabric_port_idx);
    cache.connect_port_mem_tx(fabric.port_ingress_i(fabric_port_idx))?;
    fabric.connect_port_egress_i(fabric_port_idx, cache.port_mem_rx())
}

fn connect_cache_to_memory(
    platform: &Platform,
    cache: &Rc<Cache<MemoryAccess>>,
    cache_port: Option<&str>,
    memory: &Rc<Memory<MemoryAccess>>,
) -> SimResult {
    validate_cache_mem_port(
        cache_port,
        "Cache should connect the 'mem' port to a Memory",
    )?;

    debug!(platform.entity() ; "Connect {}.mem to {}", cache, memory);
    cache.connect_port_mem_tx(memory.port_rx())?;
    memory.connect_port_tx(cache.port_mem_rx())
}

fn connect_cache_to_cache(
    platform: &Platform,
    from_cache: &Rc<Cache<MemoryAccess>>,
    from_port: Option<&str>,
    to_cache: &Rc<Cache<MemoryAccess>>,
    to_port: Option<&str>,
) -> SimResult {
    if from_port.is_some_and(|port| port != "mem") || to_port.is_some_and(|port| port != "dev") {
        return sim_error!(
            "When connecting Cache to Cache, connect 'mem' to 'dev' (or simply don't specify ports)"
        );
    }

    debug!(platform.entity() ; "Connect {}.mem to {}.dev", from_cache, to_cache);
    from_cache.connect_port_mem_tx(to_cache.port_dev_rx())?;
    to_cache.connect_port_dev_tx(from_cache.port_mem_rx())
}

fn connect_memory_to_fabric(
    platform: &Platform,
    memory: &Rc<Memory<MemoryAccess>>,
    fabric: &Rc<dyn Fabric<MemoryAccess>>,
    fabric_port_idx: usize,
) -> SimResult {
    debug!(platform.entity() ; "Connect {} to {}.{}", memory, fabric, fabric_port_idx);
    memory.connect_port_tx(fabric.port_ingress_i(fabric_port_idx))?;
    fabric.connect_port_egress_i(fabric_port_idx, memory.port_rx())
}

fn connect_fabric_to_fabric(
    platform: &Platform,
    from_fabric: &Rc<dyn Fabric<MemoryAccess>>,
    from_port_idx: usize,
    to_fabric: &Rc<dyn Fabric<MemoryAccess>>,
    to_port_idx: usize,
) -> SimResult {
    debug!(platform.entity() ; "Connect {}.{} to {}.{}", from_fabric, from_port_idx, to_fabric, to_port_idx);
    from_fabric.connect_port_egress_i(from_port_idx, to_fabric.port_ingress_i(to_port_idx))?;
    to_fabric.connect_port_egress_i(to_port_idx, from_fabric.port_ingress_i(from_port_idx))
}

fn validate_cache_dev_port(port: Option<&str>) -> SimResult {
    if port.is_some_and(|port| port != "dev") {
        return sim_error!("PEs can only connect to the 'dev' port on the Cache");
    }
    Ok(())
}

fn validate_cache_mem_port(port: Option<&str>, message: &str) -> SimResult {
    if port.is_some_and(|port| port != "mem") {
        return sim_error!("{message}");
    }
    Ok(())
}
