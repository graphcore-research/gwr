// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Protocol normalization for source-independent fabric address affinity.
use std::collections::HashMap;
use std::rc::Rc;

use gwr_engine::traits::Routable;
use gwr_engine::types::{AccessType, SimError};
use gwr_models::cache::traits::CoherentAccess;
use gwr_models::fabric::paths::{FabricAddressKey, FabricPathConfig};
use gwr_models::fabric::routed::FabricAddressProvider;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::traits::AccessMemory;

use crate::types::PlatformConfig;

pub(crate) fn memory_address_provider(
    cfg: &PlatformConfig,
    paths: &FabricPathConfig,
) -> Result<FabricAddressProvider<MemoryAccess>, SimError> {
    let ids = crate::assign_device_ids(cfg)?;
    let memories: HashMap<_, _> = cfg
        .memories
        .iter()
        .flatten()
        .enumerate()
        .map(|(index, m)| {
            (
                ids[&m.name].0,
                (index as u64, m.base_address, m.config.capacity_bytes),
            )
        })
        .collect();
    let count = paths.count;
    let shift = paths.selection.address_shift();
    Ok(Rc::new(move |packet| {
        let memory = match packet.access_type() {
            AccessType::ReadResponse | AccessType::WriteNonPostedResponse => packet.src_device().0,
            _ => packet.dst_device().0,
        };
        // Coherence packets route between directories and caches while their
        // address still identifies the physical backing memory.
        let region = if packet.coherence_op().is_some() {
            memories.values().find(|(_, base, capacity)| {
                u128::from(packet.dst_addr()) >= u128::from(*base)
                    && u128::from(packet.dst_addr()) < u128::from(*base) + u128::from(*capacity)
            })
        } else {
            memories.get(&memory)
        };
        let &(memory_index, base, capacity) = region
            .ok_or_else(|| SimError(format!("fabric packet has unknown final memory {memory}")))?;
        let offset = packet
            .dst_addr()
            .checked_sub(base)
            .ok_or_else(|| SimError("fabric memory address precedes its physical region".into()))?;
        let end = u128::from(offset) + packet.access_size_bytes().max(1) as u128;
        if end > u128::from(capacity) {
            return Err(SimError(
                "fabric memory access exceeds its physical region".into(),
            ));
        }
        if count > 1 && u128::from(offset) >> shift != (end - 1) >> shift {
            return Err(SimError(
                "fabric memory packet crosses its address-affinity region".into(),
            ));
        }
        Ok(FabricAddressKey {
            memory_index,
            offset_in_memory: offset,
        })
    }))
}

/// Check transport capacity only on request paths reachable from an issuer.
pub(crate) fn validate_geometry(
    cfg: &PlatformConfig,
    effective: &crate::builder::EffectiveConfigs,
) -> gwr_engine::types::SimResult {
    use std::collections::HashSet;
    let graph = crate::memory_mapping::request_graph(cfg)?;
    let walk = |source: &str| {
        let mut pending = vec![source.to_string()];
        let mut visited = HashSet::new();
        while let Some(node) = pending.pop() {
            if !visited.insert(node.clone()) {
                continue;
            }
            if node != source && (node.starts_with("pe.") || node.starts_with("mem.")) {
                continue;
            }
            pending.extend(graph.get(&node).into_iter().flatten().cloned());
        }
        visited
    };
    let mut sources = Vec::new();
    let mut cache_headers = HashMap::<String, usize>::new();
    for (pe, config) in cfg
        .processing_elements
        .iter()
        .flatten()
        .zip(&effective.processing_elements)
    {
        for source in [format!("pe.{}", pe.name), format!("pe.{}.dma", pe.name)] {
            for node in walk(&source) {
                if node.starts_with("cache.") {
                    cache_headers
                        .entry(node)
                        .and_modify(|n| *n = (*n).max(config.overhead_size_bytes))
                        .or_insert(config.overhead_size_bytes);
                }
            }
            // Writes can pass through caches without being split into cache
            // lines.
            sources.push((source, config.lsu_access_bytes, config.overhead_size_bytes));
        }
    }
    for (cache, config) in cfg.caches.iter().flatten().zip(&effective.caches) {
        let header = cache_headers
            .get(&format!("cache.{}.dev", cache.name))
            .copied()
            .unwrap_or(0);
        sources.push((
            format!("cache.{}.mem", cache.name),
            config.line_size_bytes(),
            header,
        ));
    }
    for (source, bytes, header) in sources {
        let transport = bytes
            .checked_add(header)
            .ok_or_else(|| SimError("fabric transport size overflows".into()))?;
        let visited = walk(&source);
        for fabric in cfg
            .fabrics
            .iter()
            .flatten()
            .filter(|f| matches!(f.kind, crate::types::FabricKind::Routed))
        {
            if !visited.contains(&format!("fabric.{}", fabric.name)) {
                continue;
            }
            let paths = fabric.config.paths.clone().unwrap_or_default();
            if paths.count > 1
                && !(1u64 << paths.selection.address_shift()).is_multiple_of(bytes as u64)
            {
                return Err(SimError(format!(
                    "Fabric '{}': {bytes}-byte fetch at '{source}' cannot fit its address-affinity region",
                    fabric.name
                )));
            }
            let rx = fabric
                .config
                .rx_buffer_bytes
                .unwrap_or(crate::builder::DEFAULT_FABRIC_RX_BUFFER_BYTES);
            let tx = fabric
                .config
                .tx_buffer_bytes
                .unwrap_or(crate::builder::DEFAULT_FABRIC_TX_BUFFER_BYTES);
            if transport > paths.queue_bytes || transport > rx || transport > tx {
                return Err(SimError(format!(
                    "Fabric '{}': queue cannot admit {transport}-byte transport packet ({bytes}-byte fetch plus {header}-byte header) at '{source}'",
                    fabric.name
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use gwr_engine::engine::Engine;
    use gwr_engine::types::DeviceId;

    use super::*;
    #[test]
    fn coherence_keys_use_backing_addresses_for_cache_and_manager_routes() {
        use gwr_models::cache::coherency_manager::CoherenceOp;
        let cfg: PlatformConfig = yaml_serde::from_str(
            "memory_maps: []\nmemories: [{name: memory, kind: hbm, base_address: 4096, config: {capacity_bytes: 4096}}]",
        ).unwrap();
        let provider = memory_address_provider(
            &cfg,
            &FabricPathConfig {
                count: 4,
                ..Default::default()
            },
        )
        .unwrap();
        let engine = Engine::default();
        let key = FabricAddressKey {
            memory_index: 0,
            offset_in_memory: 64,
        };
        for (op, access, source, destination) in [
            (CoherenceOp::SharedRead, AccessType::ReadRequest, 100, 101),
            (CoherenceOp::GrantShared, AccessType::ReadResponse, 101, 100),
            (CoherenceOp::Invalidate, AccessType::Control, 101, 102),
            (CoherenceOp::InvalidateAck, AccessType::Control, 102, 101),
        ] {
            let packet = MemoryAccess::new(
                engine.top(),
                access,
                32,
                4160,
                0,
                DeviceId(destination),
                DeviceId(source),
                0,
            )
            .with_coherence_op(Some(op));
            assert_eq!(provider(&packet).unwrap(), key);
        }
    }

    #[test]
    fn canonical_keys_ignore_source_and_normalize_replies() {
        let cfg:PlatformConfig=yaml_serde::from_str("memory_maps: []\nmemories: [{name: memory, kind: hbm, base_address: 4096, config: {capacity_bytes: 4096}}]").unwrap();
        let provider = memory_address_provider(
            &cfg,
            &FabricPathConfig {
                count: 4,
                ..Default::default()
            },
        )
        .unwrap();
        let e = Engine::default();
        let key = FabricAddressKey {
            memory_index: 0,
            offset_in_memory: 64,
        };
        for pe in [100, 101] {
            let request = MemoryAccess::new(
                e.top(),
                AccessType::ReadRequest,
                64,
                4160,
                0,
                DeviceId(0),
                DeviceId(pe),
                0,
            );
            let reply = MemoryAccess::new(
                e.top(),
                AccessType::ReadResponse,
                64,
                4160,
                0,
                DeviceId(pe),
                DeviceId(0),
                0,
            );
            assert_eq!(provider(&request).unwrap(), key);
            assert_eq!(provider(&reply).unwrap(), key);
        }
        let crossing = MemoryAccess::new(
            e.top(),
            AccessType::ReadRequest,
            64,
            4161,
            0,
            DeviceId(0),
            DeviceId(100),
            0,
        );
        assert!(provider(&crossing).is_err());
        let invalid = MemoryAccess::new(
            e.top(),
            AccessType::ReadRequest,
            64,
            8192,
            0,
            DeviceId(0),
            DeviceId(100),
            0,
        );
        assert!(provider(&invalid).is_err());
    }
}
