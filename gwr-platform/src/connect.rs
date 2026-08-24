// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::fabric::Fabric;
use gwr_models::memory::Memory;
use gwr_models::memory::cache::Cache;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::processing_element::ProcessingElement;
use gwr_track::debug;
use gwr_track::entity::GetEntity;

use crate::Platform;
use crate::connection_id::{CachePortId, ConnectionEndpointId, parse_connection_endpoint_id};
use crate::types::PlatformConfig;

pub enum PortId<'a> {
    Pe {
        pe: &'a Rc<ProcessingElement>,
    },
    Cache {
        cache: &'a Rc<Cache<MemoryAccess>>,
        port: Option<CachePortId>,
    },
    Mem {
        memory: &'a Rc<Memory<MemoryAccess>>,
    },
    FabricTile {
        fabric: &'a Rc<dyn Fabric<MemoryAccess>>,
        port_idx: usize,
    },
}

pub fn parse_port_id<'a>(platform: &'a Platform, s: &str) -> Result<PortId<'a>, SimError> {
    let endpoint = parse_connection_endpoint_id(s)?;
    resolve_port_id(platform, &endpoint)
}

fn resolve_port_id<'a>(
    platform: &'a Platform,
    endpoint: &ConnectionEndpointId,
) -> Result<PortId<'a>, SimError> {
    Ok(match endpoint {
        ConnectionEndpointId::Pe { name } => PortId::Pe {
            pe: platform.pe(name)?,
        },
        ConnectionEndpointId::Cache { name, port } => PortId::Cache {
            cache: platform.cache(name)?,
            port: port.clone(),
        },
        ConnectionEndpointId::Mem { name } => PortId::Mem {
            memory: platform.memory(name)?,
        },
        ConnectionEndpointId::FabricPort {
            fabric,
            column,
            row,
            port,
        } => {
            let fabric = platform.fabric(fabric)?;
            let port_idx = fabric.col_row_port_to_fabric_port_index(*column, *row, *port);
            PortId::FabricTile { fabric, port_idx }
        }
    })
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

            let from = parse_port_id(platform, &c.connect[0])?;
            let to = parse_port_id(platform, &c.connect[1])?;
            connect_port(platform, &from, &to)?;
        }
    }
    Ok(())
}

fn connect_port(platform: &Platform, from: &PortId, to: &PortId) -> SimResult {
    match from {
        PortId::Pe { pe } => connect_pe_to(platform, pe, to),
        PortId::Cache { cache, port } => connect_cache_to(platform, cache, port.as_ref(), to),
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
        PortId::Cache { cache, port } => connect_pe_to_cache(platform, pe, cache, port.as_ref()),
        PortId::FabricTile { fabric, port_idx } => {
            connect_pe_to_fabric(platform, pe, fabric, *port_idx)
        }
        PortId::Mem { memory } => connect_pe_to_memory(platform, pe, memory),
    }
}

fn connect_cache_to(
    platform: &Platform,
    cache: &Rc<Cache<MemoryAccess>>,
    cache_port: Option<&CachePortId>,
    to: &PortId,
) -> SimResult {
    match to {
        PortId::Pe { pe } => connect_pe_to_cache(platform, pe, cache, cache_port),
        PortId::Cache {
            cache: to_cache,
            port,
        } => connect_cache_to_cache(platform, cache, cache_port, to_cache, port.as_ref()),
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
            connect_cache_to_fabric(platform, cache, port.as_ref(), fabric, fabric_port_idx)
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
        PortId::Cache { cache, port } => {
            connect_cache_to_memory(platform, cache, port.as_ref(), memory)
        }
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
    cache_port: Option<&CachePortId>,
) -> SimResult {
    if let Some(cache_port) = cache_port
        && cache_port != &CachePortId::Dev
    {
        return sim_error!("PEs can only connect to the 'dev' port on the Cache");
    }

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
    cache_port: Option<&CachePortId>,
    fabric: &Rc<dyn Fabric<MemoryAccess>>,
    fabric_port_idx: usize,
) -> SimResult {
    if let Some(cache_port) = cache_port
        && cache_port != &CachePortId::Mem
    {
        return sim_error!("Cache should connect the 'mem' port to a Fabric");
    }

    debug!(platform.entity() ; "Connect {}.mem to {}.{}", cache, fabric, fabric_port_idx);
    cache.connect_port_mem_tx(fabric.port_ingress_i(fabric_port_idx))?;
    fabric.connect_port_egress_i(fabric_port_idx, cache.port_mem_rx())
}

fn connect_cache_to_memory(
    platform: &Platform,
    cache: &Rc<Cache<MemoryAccess>>,
    cache_port: Option<&CachePortId>,
    memory: &Rc<Memory<MemoryAccess>>,
) -> SimResult {
    if let Some(cache_port) = cache_port
        && cache_port != &CachePortId::Mem
    {
        return sim_error!("Cache should connect the 'mem' port to a Memory");
    }

    debug!(platform.entity() ; "Connect {}.mem to {}", cache, memory);
    cache.connect_port_mem_tx(memory.port_rx())?;
    memory.connect_port_tx(cache.port_mem_rx())
}

fn connect_cache_to_cache(
    platform: &Platform,
    from_cache: &Rc<Cache<MemoryAccess>>,
    from_port: Option<&CachePortId>,
    to_cache: &Rc<Cache<MemoryAccess>>,
    to_port: Option<&CachePortId>,
) -> SimResult {
    if let Some(from_port) = from_port
        && from_port != &CachePortId::Mem
    {
        return sim_error!(
            "When connecting Cache to Cache, connect 'mem' to 'dev' (or simply don't specify ports)"
        );
    }

    if let Some(to_port) = to_port
        && to_port != &CachePortId::Dev
    {
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
