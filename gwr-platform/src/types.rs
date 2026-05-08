// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use byte_unit::Byte;
use clap::ValueEnum;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use serde::{Deserialize, Serialize, de};
use serde_yaml::Value;

/// Parse a value which could be an integer or a string and return u64 value
///
/// The string can be a hex string with underscores or a Byte string that
/// specifies units. Some examples are:
///  0x10000000
///  0x1000_0000
///  10B
///  10M, 10MB, 10MiB
///
/// Or specified as number of bits in which case the number returned will be the
/// number of bytes:
///  80b, 80Mb, 80Mbit
pub fn parse_u64_byte_str<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: de::Deserializer<'de>,
{
    // We need to first deserialize to a generic `Value` so that we can
    // support the case where it is already a u64.
    let value: Value = Deserialize::deserialize(deserializer)?;

    if let Some(number) = value.as_u64() {
        // It already is a u64, so simply return that
        return Ok(number);
    }

    let s = match value.as_str() {
        Some(s) => s.to_owned(),
        None => {
            return Err(de::Error::custom(format!(
                "'{value:?}': Unsupported type for Deserialize (should be u64 or String)"
            )));
        }
    };

    // Convert to lowercase in order to standardise any 0x prefix
    let lowercase = s.to_lowercase();

    if lowercase.starts_with("0x") {
        let without_underscore = lowercase.replace('_', "");
        let without_0x = without_underscore.trim_start_matches("0x");
        u64::from_str_radix(without_0x, 16)
            .map_err(|e| de::Error::custom(format!("Unable to parse {s} as hex string: {e}")))
    } else {
        // Don't ignore case so that bit (b) and Byte (B) can be distinguished
        let ignore_case = false;
        let num_bytes = Byte::parse_str(&s, ignore_case)
            .map_err(|e| de::Error::custom(format!("Unable to parse {s} as Byte string: {e}")))?;
        Ok(num_bytes.as_u64())
    }
}

/// Same as `parse_u64_byte_str` but returns a `usize`.
pub fn parse_usize_byte_str<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: de::Deserializer<'de>,
{
    match parse_u64_byte_str(deserializer) {
        Err(e) => Err(e),
        Ok(value) => Ok(value as usize),
    }
}

/// Same as `parse_u64_byte_str` but returns a `Option<u64>`.
pub fn parse_optional_u64_byte_str<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: de::Deserializer<'de>,
{
    Ok(Some(parse_u64_byte_str(deserializer)?))
}

#[derive(Debug, Deserialize)]
pub struct PlatformConfig {
    pub memory_maps: Vec<MemoryMapSection>,
    pub processing_elements: Option<Vec<ProcessingElementSection>>,
    pub caches: Option<Vec<CacheSection>>,
    pub fabrics: Option<Vec<FabricSection>>,
    pub memories: Option<Vec<MemorySection>>,
    pub connections: Option<Vec<ConnectSection>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MemoryMapSection {
    pub name: String,
    pub devices: Vec<MemoryDeviceSection>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MemoryDeviceSection {
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProcessingElementSection {
    pub name: String,
    pub memory_map: String,
    pub config: ProcessingElementConfigSection,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct ProcessingElementConfigSection {
    pub num_active_requests: Option<usize>,
    pub lsu_access_bytes: Option<usize>,
    pub overhead_size_bytes: Option<usize>,
    #[serde(default, deserialize_with = "parse_optional_u64_byte_str")]
    pub sram_bytes: Option<u64>,
    pub adds_per_tick: Option<f64>,
    pub muls_per_tick: Option<f64>,
    pub compares_per_tick: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CacheSection {
    pub name: String,
    pub config: CacheConfigSection,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct CacheConfigSection {
    pub bw_bytes_per_cycle: Option<usize>,
    pub line_size_bytes: Option<usize>,
    pub num_ways: Option<usize>,
    pub num_sets: Option<usize>,
    pub delay_ticks: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FabricSection {
    pub name: String,
    pub kind: FabricKind,
    pub columns: usize,
    pub rows: usize,
    pub fabric_ports_per_node: Option<usize>,
    pub ticks_per_hop: Option<usize>,
    pub ticks_overhead: Option<usize>,
    pub rx_buffer_entries: Option<usize>,
    pub tx_buffer_entries: Option<usize>,
    pub port_bits_per_tick: Option<usize>,
    pub routing: Option<FabricRoutingAlgorithm>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MemorySection {
    pub name: String,
    pub kind: MemoryKind,
    #[serde(deserialize_with = "parse_u64_byte_str")]
    pub base_address: u64,
    #[serde(deserialize_with = "parse_u64_byte_str")]
    pub capacity_bytes: u64,
    pub bw_bytes_per_cycle: Option<usize>,
    pub delay_ticks: Option<usize>,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum FabricKind {
    Functional,
    Routed,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    HBM,
    DDR,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ConnectSection {
    pub connect: Vec<String>,
}
