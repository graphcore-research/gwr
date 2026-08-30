// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use gwr_engine::types::SimError;
use gwr_models::processing_element::ProcessingElementConfig;
use gwr_platform::types::PlatformConfig;

use super::{PeTable, u64_from_usize};
use crate::model::{FabricSummary, PePlatformConfig, PlatformSummary};

pub(super) fn apply_platform(platform: &PlatformConfig, pes: &mut PeTable) -> Result<(), SimError> {
    let connection_coords = pe_connection_coords(platform);
    if let Some(processing_elements) = &platform.processing_elements {
        for pe in processing_elements {
            let (col, row) = connection_coords
                .get(&pe.name)
                .copied()
                .or_else(|| pe_coords(&pe.name))
                .unwrap_or((0, 0));
            let config = PePlatformConfig::from_config(&pe.memory_map, &pe.effective_config()?)?;
            let pe_index = pes.get_or_insert(pe.name.clone(), col, row);
            let entry = pes.get_mut(pe_index);
            entry.present_in_platform = true;
            entry.row = row;
            entry.col = col;
            entry.platform_config = Some(config);
        }
    }
    Ok(())
}

pub(super) fn summarize_platform(platform: &PlatformConfig) -> Result<PlatformSummary, SimError> {
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
    let fabric_rows = platform
        .fabrics
        .iter()
        .flatten()
        .map(|fabric| u64_from_usize(fabric.rows, "fabric row count"))
        .collect::<Result<Vec<_>, _>>()?;
    let fabric_cols = platform
        .fabrics
        .iter()
        .flatten()
        .map(|fabric| u64_from_usize(fabric.columns, "fabric column count"))
        .collect::<Result<Vec<_>, _>>()?;
    let coordinate_rows = coordinates
        .iter()
        .map(|(_, row)| {
            row.checked_add(1)
                .ok_or_else(|| SimError("Platform row count overflows".to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let coordinate_cols = coordinates
        .iter()
        .map(|(col, _)| {
            col.checked_add(1)
                .ok_or_else(|| SimError("Platform column count overflows".to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rows = coordinate_rows
        .into_iter()
        .chain(fabric_rows)
        .max()
        .unwrap_or(1);
    let cols = coordinate_cols
        .into_iter()
        .chain(fabric_cols)
        .max()
        .unwrap_or(1);
    let fabrics = platform
        .fabrics
        .iter()
        .flatten()
        .map(|fabric| {
            Ok(FabricSummary {
                name: fabric.name.clone(),
                rows: u64_from_usize(fabric.rows, "fabric row count")?,
                cols: u64_from_usize(fabric.columns, "fabric column count")?,
                kind: format!("{:?}", fabric.kind).to_lowercase(),
            })
        })
        .collect::<Result<_, SimError>>()?;

    Ok(PlatformSummary {
        processing_elements: u64_from_usize(processing_elements, "processing-element count")?,
        rows,
        cols,
        fabrics,
    })
}

pub(super) fn pe_coords(name: &str) -> Option<(u64, u64)> {
    let suffix = name.strip_prefix("pe_")?;
    let (col, row) = suffix.split_once('_')?;
    Some((col.parse().ok()?, row.parse().ok()?))
}

fn pe_connection_coords(platform: &PlatformConfig) -> BTreeMap<String, (u64, u64)> {
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
    fabric_coords: Option<(u64, u64)>,
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
    fabric_coords: &mut BTreeMap<String, (u64, u64)>,
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
    fabric_coords: &BTreeMap<String, (u64, u64)>,
) -> Option<(u64, u64)> {
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

impl PePlatformConfig {
    fn from_config(memory_map: &str, config: &ProcessingElementConfig) -> Result<Self, SimError> {
        Ok(Self {
            memory_map: memory_map.to_string(),
            num_active_requests: Some(u64_from_usize(
                config.num_active_requests,
                "PE active-request count",
            )?),
            lsu_access_bytes: Some(u64_from_usize(
                config.lsu_access_bytes,
                "PE LSU access size",
            )?),
            overhead_size_bytes: Some(u64_from_usize(
                config.overhead_size_bytes,
                "PE overhead size",
            )?),
            sram_bytes: Some(u64_from_usize(config.sram_bytes, "PE SRAM size")?),
            adds_per_tick: Some(config.adds_per_tick),
            muls_per_tick: Some(config.muls_per_tick),
            compares_per_tick: Some(config.compares_per_tick),
        })
    }
}
