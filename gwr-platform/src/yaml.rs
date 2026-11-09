// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! This module emits YAML manually rather than relying on a generic serializer.
//! We need robust anchor/alias output for repeated structures, and that is key
//! to keeping generated platform files readable and editable by hand.

use std::fmt::Write as _;

use serde::Serialize;
use yaml_serde::Value;

use crate::types::{
    CacheConfigSection, ComputeSelection, PlatformConfig, ProcessingElementConfigSection,
};

pub fn platform_to_yaml_str(
    platform: &PlatformConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut out = String::new();

    emit_optional_section(&mut out, emit_operation_classes(platform)?);
    emit_optional_section(&mut out, emit_compute_profiles(platform)?);
    emit_optional_section(&mut out, emit_memory_maps(platform)?);
    emit_optional_section(&mut out, emit_processing_elements(platform)?);
    emit_optional_section(&mut out, emit_fabrics(platform)?);
    emit_optional_section(&mut out, emit_caches(platform)?);
    emit_optional_section(&mut out, emit_memories(platform)?);
    emit_optional_section(&mut out, emit_connections(platform)?);
    emit_optional_section(&mut out, emit_coherency_managers(platform)?);

    Ok(out)
}

fn emit_operation_classes(
    platform: &PlatformConfig,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if platform.operation_classes.is_empty() {
        return Ok(None);
    };

    let mut classes = start_section("operation_classes")?;
    for line in yaml_serde::to_string(&platform.operation_classes)?.lines() {
        emit_line(&mut classes, line, 1)?;
    }

    Ok(Some(classes))
}

fn emit_compute_profiles(
    platform: &PlatformConfig,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if platform.compute_profiles.is_empty() {
        return Ok(None);
    };

    let mut profiles = start_section("compute_profiles")?;
    for line in yaml_serde::to_string(&platform.compute_profiles)?.lines() {
        emit_line(&mut profiles, line, 1)?;
    }

    Ok(Some(profiles))
}

fn emit_memory_maps(
    platform: &PlatformConfig,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if platform.memory_maps.is_empty() {
        return Ok(None);
    }

    let mut out = start_section("memory_maps")?;

    for memory_map in &platform.memory_maps {
        emit_line(&mut out, format_args!("- name: {}", memory_map.name), 1)?;
        if let Some(mapping) = &memory_map.address_mapping {
            emit_line(&mut out, "address_mapping:", 2)?;
            for line in yaml_serde::to_string(mapping)?.lines() {
                emit_line(&mut out, line, 3)?;
            }
        }
        emit_line(&mut out, "devices:", 2)?;
        for range in &memory_map.devices {
            emit_line(&mut out, format_args!("- {}", range), 3)?;
        }
    }
    Ok(Some(out))
}

fn emit_processing_elements(
    platform: &PlatformConfig,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(processing_elements) = &platform.processing_elements else {
        return Ok(None);
    };

    let mut out = start_section("processing_elements")?;

    let mut unique_configs: Vec<ProcessingElementConfigSection> = Vec::new();
    for pe in processing_elements {
        if !unique_configs.iter().any(|cfg| cfg == &pe.config) {
            unique_configs.push(pe.config.clone());
        }
    }
    let mut emitted_anchors = vec![false; unique_configs.len()];

    for pe in processing_elements {
        let config = &pe.config;
        let config_idx = unique_configs
            .iter()
            .position(|cfg| cfg == config)
            .ok_or("PE config not found in unique configs")?;
        let anchor = format!("pe_config_{config_idx}");

        emit_line(&mut out, format_args!("- name: {}", pe.name), 1)?;
        emit_line(&mut out, format_args!("memory_map: {}", pe.memory_map), 2)?;
        if let Some(map) = &pe.dma_memory_map {
            emit_line(&mut out, format_args!("dma_memory_map: {}", map), 2)?;
        }
        if emitted_anchors[config_idx] {
            emit_line(&mut out, format_args!("config: *{anchor}"), 2)?;
        } else {
            emitted_anchors[config_idx] = true;
            if config.num_active_requests.is_none()
                && config.lsu_access_bytes.is_none()
                && config.overhead_size_bytes.is_none()
                && config.sram_bytes.is_none()
                && config.boundary_payload_service.is_none()
                && config.local_payload_service.is_none()
                && config.block_execution.is_none()
                && config.compute.is_none()
            {
                emit_line(&mut out, format_args!("config: &{anchor} {{}}"), 2)?;
            } else {
                emit_line(&mut out, format_args!("config: &{anchor}"), 2)?;
                emit_optional_kv(
                    &mut out,
                    "num_active_requests",
                    config.num_active_requests,
                    3,
                )?;
                emit_optional_kv(&mut out, "lsu_access_bytes", config.lsu_access_bytes, 3)?;
                emit_optional_kv(
                    &mut out,
                    "overhead_size_bytes",
                    config.overhead_size_bytes,
                    3,
                )?;
                emit_optional_kv(
                    &mut out,
                    "sram_bytes",
                    config.sram_bytes.map(u64_hex_str),
                    3,
                )?;
                if let Some(service) = &config.boundary_payload_service {
                    emit_line(&mut out, "boundary_payload_service:", 3)?;
                    emit_kv(&mut out, "bytes_per_tick", service.bytes_per_tick, 4)?;
                }
                if let Some(service) = &config.local_payload_service {
                    emit_line(&mut out, "local_payload_service:", 3)?;
                    emit_kv(&mut out, "bytes_per_tick", service.bytes_per_tick, 4)?;
                }
                if let Some(block) = &config.block_execution {
                    emit_line(&mut out, "block_execution:", 3)?;
                    for line in yaml_serde::to_string(block)?.lines() {
                        emit_line(&mut out, line, 4)?;
                    }
                }
                if let Some(compute) = &config.compute {
                    match compute {
                        ComputeSelection::Named(name) => {
                            emit_kv(&mut out, "compute", serde_json::to_string(name)?, 3)?;
                        }
                        ComputeSelection::Inline(config) => {
                            emit_line(&mut out, "compute:", 3)?;
                            for line in yaml_serde::to_string(config)?.lines() {
                                emit_line(&mut out, line, 4)?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(Some(out))
}

fn emit_fabrics(platform: &PlatformConfig) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(fabrics) = &platform.fabrics else {
        return Ok(None);
    };

    let mut out = start_section("fabrics")?;

    for fabric in fabrics {
        emit_line(&mut out, format_args!("- name: {}", fabric.name), 1)?;
        emit_line(
            &mut out,
            format_args!("kind: {}", serializable_to_str(&fabric.kind)?),
            2,
        )?;
        emit_kv(&mut out, "columns", fabric.columns, 2)?;
        emit_kv(&mut out, "rows", fabric.rows, 2)?;
        let config = &fabric.config;
        if config.fabric_ports_per_node.is_none()
            && config.ticks_per_hop.is_none()
            && config.ticks_overhead.is_none()
            && config.rx_buffer_bytes.is_none()
            && config.tx_buffer_bytes.is_none()
            && config.port_bits_per_tick.is_none()
            && config.internal_link_bits_per_tick.is_none()
            && config.routing.is_none()
            && config.port_selection.is_none()
        {
            emit_line(&mut out, "config: {}", 2)?;
            continue;
        }
        emit_line(&mut out, "config:", 2)?;
        emit_optional_kv(
            &mut out,
            "fabric_ports_per_node",
            config.fabric_ports_per_node,
            3,
        )?;
        emit_optional_kv(&mut out, "ticks_per_hop", config.ticks_per_hop, 3)?;
        emit_optional_kv(&mut out, "ticks_overhead", config.ticks_overhead, 3)?;
        emit_optional_kv(&mut out, "rx_buffer_bytes", config.rx_buffer_bytes, 3)?;
        emit_optional_kv(&mut out, "tx_buffer_bytes", config.tx_buffer_bytes, 3)?;
        emit_optional_kv(&mut out, "port_bits_per_tick", config.port_bits_per_tick, 3)?;
        emit_optional_kv(
            &mut out,
            "internal_link_bits_per_tick",
            config.internal_link_bits_per_tick,
            3,
        )?;
        if let Some(routing) = config.routing {
            emit_line(
                &mut out,
                format_args!("routing: {}", serializable_to_str(&routing)?),
                3,
            )?;
        }
        if let Some(port_selection) = fabric.config.port_selection {
            emit_line(
                &mut out,
                format_args!("port_selection: {}", serializable_to_str(&port_selection)?),
                3,
            )?;
        }
    }
    Ok(Some(out))
}

fn emit_caches(platform: &PlatformConfig) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(caches) = &platform.caches else {
        return Ok(None);
    };

    let mut out = start_section("caches")?;

    let mut unique_configs: Vec<CacheConfigSection> = Vec::new();
    for cache in caches {
        if !unique_configs.iter().any(|cfg| cfg == &cache.config) {
            unique_configs.push(cache.config.clone());
        }
    }
    let mut emitted_anchors = vec![false; unique_configs.len()];

    for cache in caches {
        let config_idx = unique_configs
            .iter()
            .position(|cfg| cfg == &cache.config)
            .ok_or("cache config not found in unique configs")?;
        let anchor = format!("cache_config_{config_idx}");
        let config = &cache.config;

        emit_line(&mut out, format_args!("- name: {}", cache.name), 1)?;
        if let Some(names) = &cache.coherency_managers {
            emit_line(&mut out, "coherency_managers:", 2)?;
            for name in names {
                emit_line(&mut out, format_args!("- {name}"), 3)?;
            }
        }
        if emitted_anchors[config_idx] {
            emit_line(&mut out, format_args!("config: *{anchor}"), 2)?;
        } else {
            emitted_anchors[config_idx] = true;
            if config.bw_bytes_per_tick.is_none()
                && config.line_size_bytes.is_none()
                && config.max_inflight_parents.is_none()
                && config.max_inflight_fills.is_none()
                && config.num_ways.is_none()
                && config.num_sets.is_none()
                && config.delay_ticks.is_none()
            {
                emit_line(&mut out, format_args!("config: &{anchor} {{}}"), 2)?;
            } else {
                emit_line(&mut out, format_args!("config: &{anchor}"), 2)?;
                emit_optional_kv(&mut out, "bw_bytes_per_tick", config.bw_bytes_per_tick, 3)?;
                emit_optional_kv(&mut out, "line_size_bytes", config.line_size_bytes, 3)?;
                emit_optional_kv(
                    &mut out,
                    "max_inflight_parents",
                    config.max_inflight_parents,
                    3,
                )?;
                emit_optional_kv(&mut out, "max_inflight_fills", config.max_inflight_fills, 3)?;
                emit_optional_kv(&mut out, "num_ways", config.num_ways, 3)?;
                emit_optional_kv(&mut out, "num_sets", config.num_sets, 3)?;
                emit_optional_kv(&mut out, "delay_ticks", config.delay_ticks, 3)?;
            }
        }
    }
    Ok(Some(out))
}

fn emit_memories(platform: &PlatformConfig) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(memories) = &platform.memories else {
        return Ok(None);
    };

    let mut out = start_section("memories")?;

    for memory in memories {
        emit_line(&mut out, format_args!("- name: {}", memory.name), 1)?;
        emit_line(
            &mut out,
            format_args!("kind: {}", serializable_to_str(&memory.kind)?),
            2,
        )?;
        emit_kv(
            &mut out,
            "base_address",
            u64_hex_str(memory.base_address),
            2,
        )?;
        emit_line(&mut out, "config:", 2)?;
        emit_kv(
            &mut out,
            "capacity_bytes",
            u64_hex_str(memory.config.capacity_bytes),
            3,
        )?;
        emit_optional_kv(
            &mut out,
            "bw_bytes_per_tick",
            memory.config.bw_bytes_per_tick,
            3,
        )?;
        emit_optional_kv(&mut out, "delay_ticks", memory.config.delay_ticks, 3)?;
        if let Some(service) = &memory.config.atomic_service {
            emit_line(&mut out, "atomic_service:", 3)?;
            // Use the existing serde YAML encoder for the closed carrier, then
            // indent its complete mapping. None contributes zero old bytes.
            let yaml = yaml_serde::to_string(service)?;
            for line in yaml.lines() {
                emit_line(&mut out, line, 4)?;
            }
        }
        if let Some(service) = &memory.config.arrival_aware_service {
            emit_line(&mut out, "arrival_aware_service:", 3)?;
            emit_kv(
                &mut out,
                "max_pending_requests",
                service.max_pending_requests,
                4,
            )?;
            if let Some(lanes) = &service.lanes {
                emit_line(&mut out, "lanes:", 4)?;
                emit_kv(&mut out, "interleave_bytes", lanes.interleave_bytes, 5)?;
                emit_kv(
                    &mut out,
                    "bytes_per_tick",
                    serde_json::to_string(&lanes.bytes_per_tick)?,
                    5,
                )?;
            }
        }
    }
    Ok(Some(out))
}

fn emit_connections(
    platform: &PlatformConfig,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(connections) = &platform.connections else {
        return Ok(None);
    };

    let mut out = start_section("connections")?;

    for connection in connections {
        emit_line(&mut out, "- connect:", 1)?;
        for endpoint in &connection.connect {
            if endpoint == "null" {
                emit_line(&mut out, "- 'null'", 3)?;
            } else {
                emit_line(&mut out, format_args!("- {endpoint}"), 3)?;
            }
        }
    }
    Ok(Some(out))
}

fn emit_optional_section(out: &mut String, section: Option<String>) {
    if let Some(section) = section {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&section);
    }
}

fn emit_coherency_managers(
    platform: &PlatformConfig,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(managers) = &platform.coherency_managers else {
        return Ok(None);
    };
    let mut out = start_section("coherency_managers")?;
    for manager in managers {
        emit_line(&mut out, format_args!("- name: {}", manager.name), 1)?;
        emit_kv(&mut out, "memory_map", &manager.memory_map, 2)?;
        if manager.config.line_size_bytes.is_some() || manager.config.operations_per_tick.is_some()
        {
            emit_line(&mut out, "config:", 2)?;
            emit_optional_kv(
                &mut out,
                "line_size_bytes",
                manager.config.line_size_bytes,
                3,
            )?;
            emit_optional_kv(
                &mut out,
                "operations_per_tick",
                manager.config.operations_per_tick,
                3,
            )?;
        } else {
            emit_line(&mut out, "config: {}", 2)?;
        }
    }
    Ok(Some(out))
}

fn start_section(section: &str) -> Result<String, std::fmt::Error> {
    let mut out = String::new();
    emit_line(&mut out, format_args!("{section}:"), 0)?;
    Ok(out)
}

fn emit_optional_kv<T: std::fmt::Display>(
    out: &mut String,
    key: &str,
    value: Option<T>,
    indent_level: usize,
) -> Result<(), std::fmt::Error> {
    if let Some(v) = value {
        emit_kv(out, key, v, indent_level)?;
    }
    Ok(())
}

fn emit_kv<T: std::fmt::Display>(
    out: &mut String,
    key: &str,
    value: T,
    indent_level: usize,
) -> Result<(), std::fmt::Error> {
    emit_line(out, format_args!("{key}: {value}"), indent_level)
}

fn emit_line(
    out: &mut String,
    line: impl std::fmt::Display,
    indent_level: usize,
) -> Result<(), std::fmt::Error> {
    writeln!(out, "{}{line}", "  ".repeat(indent_level))
}

fn serializable_to_str<T: Serialize>(value: &T) -> Result<String, Box<dyn std::error::Error>> {
    match yaml_serde::to_value(value)? {
        Value::String(s) => Ok(s),
        other => Err(format!("expected type to serialize a string, got: {other:?}").into()),
    }
}

/// Format a `u64` as lowercase hexadecimal with a `0x` prefix and underscores
/// inserted every 4 hex digits (grouped from the right).
///
/// Examples:
/// - `0x1000000` -> `0x100_0000`
/// - `0x100000000` -> `0x1_0000_0000`
fn u64_hex_str(v: u64) -> String {
    let hex = format!("{v:x}");
    let mut out = String::with_capacity(2 + hex.len() + ((hex.len().saturating_sub(1)) / 4));
    out.push_str("0x");

    let first_group_len = match hex.len() % 4 {
        0 => 4,
        n => n,
    };
    out.push_str(&hex[..first_group_len]);
    for chunk in hex.as_bytes()[first_group_len..].chunks(4) {
        out.push('_');
        // `hex` is a String, so each byte chunk should be UTF-8.
        out.push_str(std::str::from_utf8(chunk).expect("hex chunk should be utf-8"));
    }

    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use gwr_models::fabric::FabricPortSelection;
    use gwr_models::processing_element::compute::{CapabilityConfig, ComputeConfig, EngineConfig};

    #[test]
    fn emitted_null_endpoint_is_a_yaml_string_and_round_trips() {
        let platform = PlatformConfig {
            coherency_managers: None,
            compute_profiles: Default::default(),
            operation_classes: BTreeMap::new(),
            memory_maps: vec![test_memory_map()],
            defaults: None,
            processing_elements: None,
            caches: None,
            fabrics: None,
            memories: None,
            connections: Some(vec![ConnectSection {
                connect: vec!["null".to_string(), "cache.l1.dev".to_string()],
            }]),
        };

        let yaml = platform_to_yaml_str(&platform).unwrap();
        let parsed: yaml_serde::Value = yaml_serde::from_str(&yaml).unwrap();
        assert_eq!(
            parsed["connections"][0]["connect"][0].as_str(),
            Some("null")
        );

        let round_trip: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
        assert_eq!(round_trip.connections.unwrap()[0].connect[0], "null");
    }

    #[test]
    fn emits_distinct_pe_config_anchors_for_distinct_configs() {
        let shared_config = ProcessingElementConfigSection {
            block_execution: None,
            num_active_requests: Some(8),
            lsu_access_bytes: Some(32),
            overhead_size_bytes: None,
            sram_bytes: Some(0x0010_0000),
            boundary_payload_service: None,
            local_payload_service: None,
            compute: Some(super::ComputeSelection::Inline(alu(16.0, 4.0, 16.0))),
        };
        let unique_config = ProcessingElementConfigSection {
            block_execution: None,
            num_active_requests: Some(16),
            lsu_access_bytes: Some(64),
            overhead_size_bytes: Some(12),
            sram_bytes: Some(0x0020_0000),
            boundary_payload_service: None,
            local_payload_service: None,
            compute: Some(super::ComputeSelection::Inline(alu(32.0, 8.0, 16.0))),
        };
        let platform = PlatformConfig {
            coherency_managers: None,
            compute_profiles: Default::default(),
            operation_classes: BTreeMap::new(),
            memory_maps: vec![test_memory_map()],
            defaults: None,
            processing_elements: Some(vec![
                ProcessingElementSection {
                    name: "pe0".to_string(),
                    memory_map: "memory_map".to_string(),
                    dma_memory_map: None,
                    config: shared_config.clone(),
                },
                ProcessingElementSection {
                    name: "pe1".to_string(),
                    memory_map: "memory_map".to_string(),
                    dma_memory_map: None,
                    config: unique_config.clone(),
                },
                ProcessingElementSection {
                    name: "pe2".to_string(),
                    memory_map: "memory_map".to_string(),
                    dma_memory_map: None,
                    config: shared_config.clone(),
                },
            ]),
            caches: None,
            fabrics: None,
            memories: None,
            connections: None,
        };

        let yaml = platform_to_yaml_str(&platform).expect("yaml generation should succeed");

        assert!(yaml.contains("config: &pe_config_0"));
        assert!(yaml.contains("config: &pe_config_1"));
        assert!(yaml.contains("config: *pe_config_0"));
        assert!(!yaml.contains("*pe_config_1\n  - name: pe2"));

        let round_trip: PlatformConfig =
            yaml_serde::from_str(&yaml).expect("generated yaml should deserialize");
        let pes = round_trip
            .processing_elements
            .expect("processing elements should be present");
        assert_eq!(round_trip.memory_maps.len(), 1);
        assert_eq!(round_trip.memory_maps[0].name, "memory_map");
        assert_eq!(round_trip.memory_maps[0].devices.len(), 1);
        assert_eq!(round_trip.memory_maps[0].devices[0], "hbm0");
        assert_eq!(pes[0].memory_map, "memory_map");
        assert_eq!(pes[1].memory_map, "memory_map");
        assert_eq!(pes[2].memory_map, "memory_map");
        assert_eq!(pes[0].config, shared_config);
        assert_eq!(pes[1].config, unique_config);
        assert_eq!(pes[2].config, shared_config);
    }

    use super::platform_to_yaml_str;
    use crate::types::{
        CacheConfigSection, CacheSection, ConnectSection, FabricConfigSection, FabricKind,
        FabricSection, MemoryConfigSection, MemoryKind, MemoryMapSection, MemorySection,
        PlatformConfig, ProcessingElementConfigSection, ProcessingElementSection,
    };

    fn alu(add: f64, mul: f64, compare: f64) -> ComputeConfig {
        ComputeConfig {
            engines: BTreeMap::from([(
                "alu".into(),
                EngineConfig {
                    matrix_accumulator_capacity_bytes: None,
                    capabilities: [("add", add), ("mul", mul), ("compare", compare)]
                        .into_iter()
                        .map(|(class, ops_per_tick)| CapabilityConfig {
                            matrix_service_tile: None,
                            execution: None,
                            class: class.into(),
                            input_dtypes: None,
                            ops_per_tick,
                        })
                        .collect(),
                },
            )]),
        }
    }

    #[test]
    fn emits_empty_pe_and_cache_configs_as_inline_maps() {
        let empty_pe_config = ProcessingElementConfigSection {
            block_execution: None,
            num_active_requests: None,
            lsu_access_bytes: None,
            overhead_size_bytes: None,
            sram_bytes: None,
            boundary_payload_service: None,
            local_payload_service: None,
            compute: None,
        };
        let empty_cache_config = CacheConfigSection {
            bw_bytes_per_tick: None,
            line_size_bytes: None,
            max_inflight_parents: None,
            max_inflight_fills: None,
            num_ways: None,
            num_sets: None,
            delay_ticks: None,
        };
        let platform = PlatformConfig {
            coherency_managers: None,
            compute_profiles: Default::default(),
            operation_classes: BTreeMap::new(),
            memory_maps: vec![test_memory_map()],
            defaults: None,
            processing_elements: Some(vec![ProcessingElementSection {
                name: "pe0".to_string(),
                memory_map: "memory_map".to_string(),
                dma_memory_map: None,
                config: empty_pe_config.clone(),
            }]),
            caches: Some(vec![
                CacheSection {
                    coherency_managers: None,
                    name: "l1a".to_string(),
                    config: empty_cache_config.clone(),
                },
                CacheSection {
                    coherency_managers: None,
                    name: "l1b".to_string(),
                    config: empty_cache_config.clone(),
                },
            ]),
            fabrics: None,
            memories: None,
            connections: Some(vec![ConnectSection {
                connect: vec!["pe.pe0".to_string(), "cache.l1a.dev".to_string()],
            }]),
        };

        let yaml = platform_to_yaml_str(&platform).expect("yaml generation should succeed");

        assert!(yaml.contains("config: &pe_config_0 {}"));
        assert!(yaml.contains("config: &cache_config_0 {}"));
        assert!(yaml.contains("config: *cache_config_0"));

        let round_trip: PlatformConfig =
            yaml_serde::from_str(&yaml).expect("generated yaml should deserialize");
        let pe = &round_trip
            .processing_elements
            .expect("processing elements should be present")[0];
        let caches = round_trip.caches.expect("caches should be present");
        assert_eq!(round_trip.memory_maps[0].name, "memory_map");
        assert_eq!(pe.memory_map, "memory_map");
        assert_eq!(pe.config, empty_pe_config);
        assert_eq!(caches[0].config, empty_cache_config);
        assert_eq!(caches[1].config, empty_cache_config);
    }

    #[test]
    fn emits_fabric_and_memory_configs() {
        let fabric_config = FabricConfigSection {
            fabric_ports_per_node: Some(2),
            ticks_per_hop: Some(3),
            ticks_overhead: Some(4),
            rx_buffer_bytes: Some(5),
            tx_buffer_bytes: Some(6),
            port_bits_per_tick: Some(7),
            internal_link_bits_per_tick: None,
            routing: Some(gwr_models::fabric::node::FabricRoutingAlgorithm::RowFirst),
            port_selection: None,
        };
        let memory_config = MemoryConfigSection {
            atomic_service: None,
            capacity_bytes: 0x2000,
            bw_bytes_per_tick: Some(8),
            delay_ticks: Some(9),
            arrival_aware_service: None,
        };
        let platform = PlatformConfig {
            coherency_managers: None,
            compute_profiles: Default::default(),
            operation_classes: BTreeMap::new(),
            memory_maps: vec![test_memory_map()],
            defaults: None,
            processing_elements: None,
            caches: None,
            fabrics: Some(vec![FabricSection {
                name: "fabric0".to_string(),
                kind: FabricKind::Functional,
                columns: 2,
                rows: 3,
                config: fabric_config.clone(),
            }]),
            memories: Some(vec![MemorySection {
                name: "hbm0".to_string(),
                kind: MemoryKind::HBM,
                base_address: 0x1000,
                config: memory_config.clone(),
            }]),
            connections: None,
        };

        let yaml = platform_to_yaml_str(&platform).expect("yaml generation should succeed");

        let round_trip: PlatformConfig =
            yaml_serde::from_str(&yaml).expect("generated yaml should deserialize");
        let fabric = &round_trip.fabrics.expect("fabric should be present")[0];
        let memory = &round_trip.memories.expect("memory should be present")[0];
        assert_eq!(fabric.columns, 2);
        assert_eq!(fabric.rows, 3);
        assert_eq!(fabric.config, fabric_config);
        assert_eq!(memory.base_address, 0x1000);
        assert_eq!(memory.config, memory_config);
    }

    #[test]
    fn emits_fabric_port_selection_inside_config() {
        let only_port_selection = FabricConfigSection {
            fabric_ports_per_node: None,
            ticks_per_hop: None,
            ticks_overhead: None,
            rx_buffer_bytes: None,
            tx_buffer_bytes: None,
            port_bits_per_tick: None,
            internal_link_bits_per_tick: None,
            routing: None,
            port_selection: Some(FabricPortSelection::SourceIdModulo),
        };
        let mixed_config = FabricConfigSection {
            routing: Some(gwr_models::fabric::node::FabricRoutingAlgorithm::RowFirst),
            ..only_port_selection.clone()
        };
        let platform = PlatformConfig {
            coherency_managers: None,
            compute_profiles: Default::default(),
            operation_classes: BTreeMap::new(),
            memory_maps: vec![test_memory_map()],
            defaults: None,
            processing_elements: None,
            caches: None,
            fabrics: Some(vec![
                FabricSection {
                    name: "fabric0".to_string(),
                    kind: FabricKind::Functional,
                    columns: 2,
                    rows: 3,
                    config: only_port_selection.clone(),
                },
                FabricSection {
                    name: "fabric1".to_string(),
                    kind: FabricKind::Functional,
                    columns: 4,
                    rows: 5,
                    config: mixed_config.clone(),
                },
            ]),
            memories: None,
            connections: None,
        };

        let yaml = platform_to_yaml_str(&platform).expect("yaml generation should succeed");

        assert!(yaml.contains(
            "- name: fabric0\n    kind: functional\n    columns: 2\n    rows: 3\n    config:\n      port_selection: source-id-modulo"
        ));
        assert!(yaml.contains(
            "- name: fabric1\n    kind: functional\n    columns: 4\n    rows: 5\n    config:\n      routing: row-first\n      port_selection: source-id-modulo"
        ));

        let round_trip: PlatformConfig =
            yaml_serde::from_str(&yaml).expect("generated yaml should deserialize");
        let fabrics = round_trip.fabrics.expect("fabrics should be present");
        assert_eq!(fabrics[0].config, only_port_selection);
        assert_eq!(fabrics[1].config, mixed_config);
    }

    fn test_memory_map() -> MemoryMapSection {
        MemoryMapSection {
            address_mapping: None,
            name: "memory_map".to_string(),
            devices: vec!["hbm0".to_string()],
        }
    }
}
