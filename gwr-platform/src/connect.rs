// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::cache::Cache;
use gwr_models::fabric::Fabric;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::model::MemoryEndpoint;
use gwr_track::debug;
use gwr_track::entity::GetEntity;

use crate::Platform;
use crate::connection_id::{CachePortId, ConnectionEndpointId, parse_connection_endpoint_id};
use crate::types::PlatformConfig;
use crate::validation::{cache_connection_port, validate_port_endpoint_pair};

fn resolve_endpoint<'a>(
    platform: &'a Platform,
    endpoint: &ConnectionEndpointId,
    other: &ConnectionEndpointId,
    is_from: bool,
) -> Result<MemoryEndpoint<'a>, SimError> {
    Ok(match endpoint {
        ConnectionEndpointId::Null => unreachable!("null connections handled separately"),
        ConnectionEndpointId::CoherencyManager { name } => {
            let manager = platform.coherency_manager(name)?;
            MemoryEndpoint::detailed(|| manager.port_rx(), |p| manager.connect_port_tx(p))
        }
        ConnectionEndpointId::Pe { name } => {
            if let Some(name) = name.strip_suffix(".dma") {
                let pe = platform.pe(name)?;
                MemoryEndpoint::detailed(|| pe.dma_rx(), |p| pe.connect_dma_tx(p))
            } else {
                platform.pe_model(name)?.endpoint()
            }
        }
        ConnectionEndpointId::Mem { name } => platform.memory_model(name)?.endpoint(),
        ConnectionEndpointId::Cache { name, port } => {
            let cache = platform.cache_model(name)?;
            match cache_connection_port(port.as_ref(), other, is_from) {
                "dev" => cache.device_endpoint(),
                "mem" => cache.memory_endpoint(),
                _ => unreachable!("validated cache port"),
            }
        }
        ConnectionEndpointId::FabricPort {
            fabric,
            column,
            row,
            port,
        } => {
            let fabric = platform.fabric_model(fabric)?;
            let index = fabric.col_row_port_to_fabric_port_index(*column, *row, *port);
            fabric.endpoint(index)
        }
    })
}

// Preserve the old detailed connector's direction order, including reversed
// declarations. Cache-to-cache keeps declaration order for mem -> dev
// inference.
fn connection_rank(endpoint: &ConnectionEndpointId) -> u8 {
    match endpoint {
        ConnectionEndpointId::Null => 4,
        ConnectionEndpointId::CoherencyManager { .. } | ConnectionEndpointId::Pe { .. } => 0,
        ConnectionEndpointId::Cache { .. } => 1,
        ConnectionEndpointId::Mem { .. } => 2,
        ConnectionEndpointId::FabricPort { .. } => 3,
    }
}

pub fn connect_ports(
    engine: &Engine,
    clock: &Clock,
    platform: &Platform,
    cfg: &PlatformConfig,
) -> SimResult {
    for connection in cfg.connections.iter().flatten() {
        let [from, to] = connection.connect.as_slice() else {
            return Err(SimError(format!(
                "Invalid 'connect' with {} entries (only 2 expected)",
                connection.connect.len()
            )));
        };
        let mut a = parse_connection_endpoint_id(from)?;
        let mut b = parse_connection_endpoint_id(to)?;
        validate_port_endpoint_pair(&a, &b)?;
        if matches!(a, ConnectionEndpointId::Null) || matches!(b, ConnectionEndpointId::Null) {
            let endpoint = if matches!(a, ConnectionEndpointId::Null) {
                &b
            } else {
                &a
            };
            match endpoint {
                ConnectionEndpointId::Cache { name, port } => {
                    connect_cache_to_null(engine, clock, platform.cache(name)?, port.as_ref())?;
                }
                ConnectionEndpointId::FabricPort {
                    fabric,
                    column,
                    row,
                    port,
                } => {
                    let fabric = platform.fabric(fabric)?;
                    let index = fabric.col_row_port_to_fabric_port_index(*column, *row, *port);
                    connect_fabric_to_null(engine, clock, fabric, index)?;
                }
                _ => unreachable!("validated null connection"),
            }
            continue;
        }
        if connection_rank(&a) > connection_rank(&b) {
            std::mem::swap(&mut a, &mut b);
        }
        let left = resolve_endpoint(platform, &a, &b, true)?;
        let right = resolve_endpoint(platform, &b, &a, false)?;
        debug!(platform.entity(); "Connect {from} to {to}");
        left.connect_detailed(&right)
            .map_err(|error| SimError(format!("Connection '{from}' -> '{to}': {error}")))?;
    }
    Ok(())
}

fn connect_cache_to_null(
    engine: &Engine,
    clock: &Clock,
    cache: &Rc<Cache<MemoryAccess>>,
    port: Option<&CachePortId>,
) -> SimResult {
    match port {
        Some(CachePortId::Dev) => {
            let mut tx = OutPort::new(cache.entity(), "null_dev_tx");
            tx.connect(cache.port_dev_rx())?;
            let rx = InPort::new(engine, clock, cache.entity(), "null_dev_rx");
            cache.connect_port_dev_tx(rx.state())
        }
        Some(CachePortId::Mem) | None => {
            let mut tx = OutPort::new(cache.entity(), "null_mem_tx");
            tx.connect(cache.port_mem_rx())?;
            let rx = InPort::new(engine, clock, cache.entity(), "null_mem_rx");
            cache.connect_port_mem_tx(rx.state())
        }
    }
}

fn connect_fabric_to_null(
    engine: &Engine,
    clock: &Clock,
    fabric: &Rc<dyn Fabric<MemoryAccess>>,
    port_idx: usize,
) -> SimResult {
    let mut tx = OutPort::new(fabric.entity(), &format!("null_tx_{port_idx}"));
    tx.connect(fabric.port_ingress_i(port_idx))?;
    let rx = InPort::new(
        engine,
        clock,
        fabric.entity(),
        &format!("null_rx_{port_idx}"),
    );
    fabric.connect_port_egress_i(port_idx, rx.state())
}
