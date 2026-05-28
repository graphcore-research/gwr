// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt;

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
#[serde(deny_unknown_fields)]
pub struct PlatformConfig {
    pub memory_maps: Vec<MemoryMapSection>,
    pub defaults: Option<DefaultsSection>,
    pub processing_elements: Option<Vec<ProcessingElementSection>>,
    pub caches: Option<Vec<CacheSection>>,
    pub coherency_managers: Option<Vec<CoherencyManagerSection>>,
    pub fabrics: Option<Vec<FabricSection>>,
    pub memories: Option<Vec<MemorySection>>,
    pub connections: Option<Vec<ConnectSection>>,
}

// Defaults intentionally accepts unknown fields so users can define custom
// YAML anchors for reuse elsewhere in the platform file. Do not add
// `#[serde(deny_unknown_fields)]` here.
#[derive(Debug, Deserialize)]
pub struct DefaultsSection {
    pub pe_config: Option<ProcessingElementConfigSection>,
    pub cache_config: Option<CacheConfigSection>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MemoryMapSection {
    pub name: String,
    pub devices: Vec<MemoryDeviceSection>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MemoryDeviceSection {
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ProcessingElementSection {
    pub name: String,
    pub memory_map: String,
    pub config: ProcessingElementConfigSection,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct CacheSection {
    pub name: String,
    pub memory_map: String,
    pub coherency_manager: Option<String>,
    pub coherency_managers: Option<Vec<String>>,
    pub config: CacheConfigSection,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CacheConfigSection {
    pub bw_bytes_per_cycle: Option<usize>,
    pub line_size_bytes: Option<usize>,
    pub num_ways: Option<usize>,
    pub num_sets: Option<usize>,
    pub delay_ticks: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CoherencyManagerSection {
    pub name: String,
    pub memory_map: String,
    pub config: CoherencyManagerConfigSection,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoherencyManagerConfigSection {
    pub line_size_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct FabricSection {
    pub name: String,
    pub kind: FabricKind,
    pub columns: usize,
    pub rows: usize,
    pub fabric_ports_per_node: Option<usize>,
    pub port_devices: Option<Vec<FabricPortDevicesSection>>,
    pub ticks_per_hop: Option<usize>,
    pub ticks_overhead: Option<usize>,
    pub rx_buffer_bytes: Option<usize>,
    pub tx_buffer_bytes: Option<usize>,
    pub port_bits_per_tick: Option<usize>,
    pub routing: Option<FabricRoutingAlgorithm>,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FabricPortDevicesSection {
    pub port: FabricPortLocation,
    pub devices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabricPortLocation {
    pub column: usize,
    pub row: usize,
    pub port: usize,
}

impl fmt::Display for FabricPortLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.port == 0 {
            write!(f, "({},{})", self.column, self.row)
        } else {
            write!(f, "({},{}).{}", self.column, self.row, self.port)
        }
    }
}

impl<'de> Deserialize<'de> for FabricPortLocation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let value: Value = Deserialize::deserialize(deserializer)?;
        let s = value.as_str().ok_or_else(|| {
            de::Error::custom(format!(
                "'{value:?}': Unsupported type for fabric port (should be a string like '(0,1)')"
            ))
        })?;
        parse_fabric_port_location(s).map_err(de::Error::custom)
    }
}

fn parse_fabric_port_location(s: &str) -> Result<FabricPortLocation, String> {
    let Some(open) = s.find('(') else {
        return Err(format!("Invalid fabric port '{s}': missing '('"));
    };
    let Some(close) = s.find(')') else {
        return Err(format!("Invalid fabric port '{s}': missing ')'"));
    };
    if open != 0 || close <= open + 1 {
        return Err(format!("Invalid fabric port '{s}'"));
    }

    let coords = &s[open + 1..close];
    let mut parts = coords.split(',').map(str::trim);
    let column = parts
        .next()
        .ok_or_else(|| format!("Invalid fabric port '{s}': missing column"))?
        .parse()
        .map_err(|e| format!("Invalid fabric port '{s}' column: {e}"))?;
    let row = parts
        .next()
        .ok_or_else(|| format!("Invalid fabric port '{s}': missing row"))?
        .parse()
        .map_err(|e| format!("Invalid fabric port '{s}' row: {e}"))?;
    if parts.next().is_some() {
        return Err(format!("Invalid fabric port '{s}': too many coordinates"));
    }

    let suffix = s[close + 1..].trim();
    let port = if suffix.is_empty() {
        0
    } else {
        let Some(port_suffix) = suffix.strip_prefix('.') else {
            return Err(format!(
                "Invalid fabric port '{s}': expected optional '.<port>' suffix"
            ));
        };
        port_suffix
            .parse()
            .map_err(|e| format!("Invalid fabric port '{s}' port: {e}"))?
    };

    Ok(FabricPortLocation { column, row, port })
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct ConnectSection {
    pub connect: Vec<String>,
}
