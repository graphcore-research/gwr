// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! This module emits YAML manually rather than relying on a generic serializer.
//! We need robust anchor/alias output for repeated structures, and that is key
//! to keeping generated platform files readable and editable by hand.

use std::fmt::Write as _;

use serde::Serialize;
use serde_yaml::Value;

use crate::types::{CacheConfigSection, PlatformConfig, ProcessingElementConfigSection};

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

fn emit_line(
    out: &mut String,
    line: impl std::fmt::Display,
    indent_level: usize,
) -> Result<(), std::fmt::Error> {
    writeln!(out, "{}{line}", "  ".repeat(indent_level))
}

fn start_section(section: &str) -> Result<String, std::fmt::Error> {
    let mut out = String::new();
    emit_line(&mut out, format_args!("{section}:"), 0)?;
    Ok(out)
}

fn emit_kv<T: std::fmt::Display>(
    out: &mut String,
    key: &str,
    value: T,
    indent_level: usize,
) -> Result<(), std::fmt::Error> {
    emit_line(out, format_args!("{key}: {value}"), indent_level)
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

fn serializable_to_str<T: Serialize>(value: &T) -> Result<String, Box<dyn std::error::Error>> {
    match serde_yaml::to_value(value)? {
        Value::String(s) => Ok(s),
        other => Err(format!("expected type to serialize a string, got: {other:?}").into()),
    }
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
        emit_line(&mut out, "devices:", 2)?;
        for range in &memory_map.devices {
            emit_line(&mut out, format_args!("- name: {}", range.name), 3)?;
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
        if emitted_anchors[config_idx] {
            emit_line(&mut out, format_args!("config: *{anchor}"), 2)?;
        } else {
            emitted_anchors[config_idx] = true;
            if config.num_active_requests.is_none()
                && config.lsu_access_bytes.is_none()
                && config.overhead_size_bytes.is_none()
                && config.sram_bytes.is_none()
                && config.adds_per_tick.is_none()
                && config.muls_per_tick.is_none()
                && config.compares_per_tick.is_none()
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
                emit_optional_kv(&mut out, "adds_per_tick", config.adds_per_tick, 3)?;
                emit_optional_kv(&mut out, "muls_per_tick", config.muls_per_tick, 3)?;
                emit_optional_kv(&mut out, "compares_per_tick", config.compares_per_tick, 3)?;
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
        emit_optional_kv(
            &mut out,
            "fabric_ports_per_node",
            fabric.fabric_ports_per_node,
            2,
        )?;
        emit_optional_kv(&mut out, "ticks_per_hop", fabric.ticks_per_hop, 2)?;
        emit_optional_kv(&mut out, "ticks_overhead", fabric.ticks_overhead, 2)?;
        emit_optional_kv(&mut out, "rx_buffer_bytes", fabric.rx_buffer_bytes, 2)?;
        emit_optional_kv(&mut out, "tx_buffer_bytes", fabric.tx_buffer_bytes, 2)?;
        emit_optional_kv(&mut out, "port_bits_per_tick", fabric.port_bits_per_tick, 2)?;
        if let Some(routing) = fabric.routing {
            emit_line(
                &mut out,
                format_args!("routing: {}", serializable_to_str(&routing)?),
                2,
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
        if emitted_anchors[config_idx] {
            emit_line(&mut out, format_args!("config: *{anchor}"), 2)?;
        } else {
            emitted_anchors[config_idx] = true;
            if config.bw_bytes_per_cycle.is_none()
                && config.line_size_bytes.is_none()
                && config.num_ways.is_none()
                && config.num_sets.is_none()
                && config.delay_ticks.is_none()
            {
                emit_line(&mut out, format_args!("config: &{anchor} {{}}"), 2)?;
            } else {
                emit_line(&mut out, format_args!("config: &{anchor}"), 2)?;
                emit_optional_kv(&mut out, "bw_bytes_per_cycle", config.bw_bytes_per_cycle, 3)?;
                emit_optional_kv(&mut out, "line_size_bytes", config.line_size_bytes, 3)?;
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
        emit_kv(
            &mut out,
            "capacity_bytes",
            u64_hex_str(memory.capacity_bytes),
            2,
        )?;
        emit_optional_kv(&mut out, "bw_bytes_per_cycle", memory.bw_bytes_per_cycle, 2)?;
        emit_optional_kv(&mut out, "delay_ticks", memory.delay_ticks, 2)?;
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
            emit_line(&mut out, format_args!("- {endpoint}"), 3)?;
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

pub fn platform_to_yaml_str(
    platform: &PlatformConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut out = String::new();

    emit_optional_section(&mut out, emit_memory_maps(platform)?);
    emit_optional_section(&mut out, emit_processing_elements(platform)?);
    emit_optional_section(&mut out, emit_fabrics(platform)?);
    emit_optional_section(&mut out, emit_caches(platform)?);
    emit_optional_section(&mut out, emit_memories(platform)?);
    emit_optional_section(&mut out, emit_connections(platform)?);

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::platform_to_yaml_str;
    use crate::types::{
        CacheConfigSection, CacheSection, ConnectSection, MemoryDeviceSection, MemoryMapSection,
        PlatformConfig, ProcessingElementConfigSection, ProcessingElementSection,
    };

    fn test_memory_map() -> MemoryMapSection {
        MemoryMapSection {
            name: "memory_map".to_string(),
            devices: vec![MemoryDeviceSection {
                name: "hbm0".to_string(),
            }],
        }
    }

    #[test]
    fn emits_distinct_pe_config_anchors_for_distinct_configs() {
        let shared_config = ProcessingElementConfigSection {
            num_active_requests: Some(8),
            lsu_access_bytes: Some(32),
            overhead_size_bytes: None,
            sram_bytes: Some(0x0010_0000),
            adds_per_tick: Some(16.0),
            muls_per_tick: Some(4.0),
            compares_per_tick: None,
        };
        let unique_config = ProcessingElementConfigSection {
            num_active_requests: Some(16),
            lsu_access_bytes: Some(64),
            overhead_size_bytes: Some(12),
            sram_bytes: Some(0x0020_0000),
            adds_per_tick: Some(32.0),
            muls_per_tick: Some(8.0),
            compares_per_tick: Some(16.0),
        };
        let platform = PlatformConfig {
            memory_maps: vec![test_memory_map()],
            defaults: None,
            processing_elements: Some(vec![
                ProcessingElementSection {
                    name: "pe0".to_string(),
                    memory_map: "memory_map".to_string(),
                    config: shared_config.clone(),
                },
                ProcessingElementSection {
                    name: "pe1".to_string(),
                    memory_map: "memory_map".to_string(),
                    config: unique_config.clone(),
                },
                ProcessingElementSection {
                    name: "pe2".to_string(),
                    memory_map: "memory_map".to_string(),
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
            serde_yaml::from_str(&yaml).expect("generated yaml should deserialize");
        let pes = round_trip
            .processing_elements
            .expect("processing elements should be present");
        assert_eq!(round_trip.memory_maps.len(), 1);
        assert_eq!(round_trip.memory_maps[0].name, "memory_map");
        assert_eq!(round_trip.memory_maps[0].devices.len(), 1);
        assert_eq!(round_trip.memory_maps[0].devices[0].name, "hbm0");
        assert_eq!(pes[0].memory_map, "memory_map");
        assert_eq!(pes[1].memory_map, "memory_map");
        assert_eq!(pes[2].memory_map, "memory_map");
        assert_eq!(pes[0].config, shared_config);
        assert_eq!(pes[1].config, unique_config);
        assert_eq!(pes[2].config, shared_config);
    }

    #[test]
    fn emits_empty_pe_and_cache_configs_as_inline_maps() {
        let empty_pe_config = ProcessingElementConfigSection {
            num_active_requests: None,
            lsu_access_bytes: None,
            overhead_size_bytes: None,
            sram_bytes: None,
            adds_per_tick: None,
            muls_per_tick: None,
            compares_per_tick: None,
        };
        let empty_cache_config = CacheConfigSection {
            bw_bytes_per_cycle: None,
            line_size_bytes: None,
            num_ways: None,
            num_sets: None,
            delay_ticks: None,
        };
        let platform = PlatformConfig {
            memory_maps: vec![test_memory_map()],
            defaults: None,
            processing_elements: Some(vec![ProcessingElementSection {
                name: "pe0".to_string(),
                memory_map: "memory_map".to_string(),
                config: empty_pe_config.clone(),
            }]),
            caches: Some(vec![
                CacheSection {
                    name: "l1a".to_string(),
                    config: empty_cache_config.clone(),
                },
                CacheSection {
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
            serde_yaml::from_str(&yaml).expect("generated yaml should deserialize");
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
}
