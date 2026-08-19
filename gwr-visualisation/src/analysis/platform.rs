// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use gwr_platform::builder::{
    DEFAULT_PE_ADDS_PER_TICK, DEFAULT_PE_COMPARES_PER_TICK, DEFAULT_PE_LSU_ACCESS_BYTES,
    DEFAULT_PE_MULS_PER_TICK, DEFAULT_PE_NUM_ACTIVE_REQUESTS, DEFAULT_PE_OVERHEAD_SIZE_BYTES,
    DEFAULT_PE_SRAM_BYTES,
};
use gwr_platform::types::{PlatformConfig, ProcessingElementConfigSection};

use crate::model::{FabricSummary, PePlatformConfig, PeSummary, PlatformSummary};

pub(super) fn apply_platform(
    platform: &PlatformConfig,
    pes_by_name: &mut BTreeMap<String, PeSummary>,
) {
    let connection_coords = pe_connection_coords(platform);
    if let Some(processing_elements) = &platform.processing_elements {
        for pe in processing_elements {
            let (col, row) = connection_coords
                .get(&pe.name)
                .copied()
                .or_else(|| pe_coords(&pe.name))
                .unwrap_or((0, 0));
            let config = PePlatformConfig::from_config(&pe.memory_map, &pe.config);
            let entry = pes_by_name
                .entry(pe.name.clone())
                .or_insert_with(|| PeSummary::new(pe.name.clone(), col, row));
            entry.present_in_platform = true;
            entry.row = row;
            entry.col = col;
            entry.platform_config = Some(config);
        }
    }
}

fn pe_connection_coords(platform: &PlatformConfig) -> BTreeMap<String, (usize, usize)> {
    let mut connections = BTreeMap::<String, BTreeSet<String>>::new();
    let mut pe_nodes = BTreeMap::new();
    let mut fabric_coords = BTreeMap::new();
    for connection in platform.connections.iter().flatten() {
        let [left, right] = connection.connect.as_slice() else {
            continue;
        };
        let Some(left) = topology_endpoint(left) else {
            continue;
        };
        let Some(right) = topology_endpoint(right) else {
            continue;
        };
        record_endpoint(&left, &mut pe_nodes, &mut fabric_coords);
        record_endpoint(&right, &mut pe_nodes, &mut fabric_coords);
        connections
            .entry(left.key.clone())
            .or_default()
            .insert(right.key.clone());
        connections.entry(right.key).or_default().insert(left.key);
    }

    pe_nodes
        .into_iter()
        .filter_map(|(pe_name, node)| {
            connected_fabric_coord(&node, &connections, &fabric_coords)
                .map(|coords| (pe_name, coords))
        })
        .collect()
}

struct TopologyEndpoint {
    key: String,
    pe_name: Option<String>,
    fabric_coords: Option<(usize, usize)>,
}

fn topology_endpoint(endpoint: &str) -> Option<TopologyEndpoint> {
    if let Some(pe_name) = endpoint.strip_prefix("pe.") {
        return (!pe_name.contains('.')).then(|| TopologyEndpoint {
            key: endpoint.to_string(),
            pe_name: Some(pe_name.to_string()),
            fabric_coords: None,
        });
    }
    if let Some(cache) = endpoint.strip_prefix("cache.") {
        let cache_name = cache.split('.').next()?;
        return Some(TopologyEndpoint {
            key: format!("cache.{cache_name}"),
            pe_name: None,
            fabric_coords: None,
        });
    }

    let fabric = endpoint.strip_prefix("fabric.")?;
    let (fabric_name, coordinates) = fabric.split_once("@(")?;
    let (coordinates, suffix) = coordinates.split_once(')')?;
    if !suffix.is_empty() && !suffix.starts_with('.') {
        return None;
    }
    let (col, row) = coordinates.split_once(',')?;
    let coords = (col.parse().ok()?, row.parse().ok()?);
    Some(TopologyEndpoint {
        key: format!("fabric.{fabric_name}@({coordinates})"),
        pe_name: None,
        fabric_coords: Some(coords),
    })
}

fn record_endpoint(
    endpoint: &TopologyEndpoint,
    pe_nodes: &mut BTreeMap<String, String>,
    fabric_coords: &mut BTreeMap<String, (usize, usize)>,
) {
    if let Some(pe_name) = &endpoint.pe_name {
        pe_nodes.insert(pe_name.clone(), endpoint.key.clone());
    }
    if let Some(coords) = endpoint.fabric_coords {
        fabric_coords.insert(endpoint.key.clone(), coords);
    }
}

fn connected_fabric_coord(
    start: &str,
    connections: &BTreeMap<String, BTreeSet<String>>,
    fabric_coords: &BTreeMap<String, (usize, usize)>,
) -> Option<(usize, usize)> {
    let mut pending = VecDeque::from([start.to_string()]);
    let mut visited = BTreeSet::new();
    while let Some(node) = pending.pop_front() {
        if !visited.insert(node.clone()) {
            continue;
        }
        if let Some(coords) = fabric_coords.get(&node) {
            return Some(*coords);
        }
        pending.extend(connections.get(&node).into_iter().flatten().cloned());
    }
    None
}

pub(super) fn summarize_platform(platform: &PlatformConfig) -> PlatformSummary {
    let connection_coords = pe_connection_coords(platform);
    let coordinates = platform
        .processing_elements
        .iter()
        .flatten()
        .filter_map(|pe| {
            connection_coords
                .get(&pe.name)
                .copied()
                .or_else(|| pe_coords(&pe.name))
        })
        .collect::<Vec<_>>();
    let processing_elements = platform.processing_elements.as_ref().map_or(0, Vec::len);
    let fabric_rows = platform.fabrics.iter().flatten().map(|fabric| fabric.rows);
    let fabric_cols = platform
        .fabrics
        .iter()
        .flatten()
        .map(|fabric| fabric.columns);
    let rows = coordinates
        .iter()
        .map(|(_, row)| row.saturating_add(1))
        .chain(fabric_rows)
        .max()
        .unwrap_or(1);
    let cols = coordinates
        .iter()
        .map(|(col, _)| col.saturating_add(1))
        .chain(fabric_cols)
        .max()
        .unwrap_or(1);
    let fabrics = platform
        .fabrics
        .iter()
        .flatten()
        .map(|fabric| FabricSummary {
            name: fabric.name.clone(),
            rows: fabric.rows,
            cols: fabric.columns,
            kind: format!("{:?}", fabric.kind).to_lowercase(),
        })
        .collect();

    PlatformSummary {
        processing_elements,
        rows,
        cols,
        fabrics,
    }
}

pub(super) fn pe_coords(name: &str) -> Option<(usize, usize)> {
    let suffix = name.strip_prefix("pe_")?;
    let (col, row) = suffix.split_once('_')?;
    Some((col.parse().ok()?, row.parse().ok()?))
}

impl PePlatformConfig {
    fn from_config(memory_map: &str, config: &ProcessingElementConfigSection) -> Self {
        Self {
            memory_map: memory_map.to_string(),
            num_active_requests: Some(
                config
                    .num_active_requests
                    .unwrap_or(DEFAULT_PE_NUM_ACTIVE_REQUESTS),
            ),
            lsu_access_bytes: Some(
                config
                    .lsu_access_bytes
                    .unwrap_or(DEFAULT_PE_LSU_ACCESS_BYTES),
            ),
            overhead_size_bytes: Some(
                config
                    .overhead_size_bytes
                    .unwrap_or(DEFAULT_PE_OVERHEAD_SIZE_BYTES),
            ),
            sram_bytes: Some(config.sram_bytes.unwrap_or(DEFAULT_PE_SRAM_BYTES)),
            adds_per_tick: Some(config.adds_per_tick.unwrap_or(DEFAULT_PE_ADDS_PER_TICK)),
            muls_per_tick: Some(config.muls_per_tick.unwrap_or(DEFAULT_PE_MULS_PER_TICK)),
            compares_per_tick: Some(
                config
                    .compares_per_tick
                    .unwrap_or(DEFAULT_PE_COMPARES_PER_TICK),
            ),
        }
    }
}
