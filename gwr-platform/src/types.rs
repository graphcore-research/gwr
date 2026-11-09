// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use byte_unit::Byte;
use clap::ValueEnum;
use gwr_models::fabric::FabricPortSelection;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::processing_element::compute::{ComputeConfig, OperationClass};
use serde::{Deserialize, Serialize, de};
use yaml_serde::Value;

/// Same as `parse_u64_byte_str` but returns a `usize`.
pub fn parse_usize_byte_str<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: de::Deserializer<'de>,
{
    let value = parse_u64_byte_str(deserializer)?;
    usize::try_from(value).map_err(|error| {
        de::Error::custom(format!("{value} cannot be represented as usize: {error}"))
    })
}

/// Same as `parse_u64_byte_str` but returns a `Option<u64>`.
pub fn parse_optional_u64_byte_str<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: de::Deserializer<'de>,
{
    Ok(Some(parse_u64_byte_str(deserializer)?))
}

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformConfig {
    #[serde(default)]
    pub operation_classes: BTreeMap<String, OperationClass>,
    #[serde(default)]
    pub compute_profiles: BTreeMap<String, ComputeConfig>,
    pub memory_maps: Vec<MemoryMapSection>,
    pub defaults: Option<DefaultsSection>,
    pub processing_elements: Option<Vec<ProcessingElementSection>>,
    pub caches: Option<Vec<CacheSection>>,
    pub coherency_managers: Option<Vec<CoherencyManagerSection>>,
    pub fabrics: Option<Vec<FabricSection>>,
    pub memories: Option<Vec<MemorySection>>,
    pub connections: Option<Vec<ConnectSection>>,
}

#[derive(Debug, Deserialize)]
pub struct DefaultsSection {
    pub pe_config: Option<ProcessingElementConfigSection>,
    pub cache_config: Option<CacheConfigSection>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MemoryMapSection {
    pub name: String,
    /// Optional round-robin placement; omission preserves contiguous mapping.
    #[serde(default)]
    pub address_mapping: Option<AddressMappingSection>,
    /// Names of memories included in this map.
    pub devices: Vec<String>,
}

/// Placement policy retained in platform files even when striping is disabled.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AddressMappingSection {
    pub mode: AddressMappingMode,
    pub stripe_bytes: u64,
    pub groups: Vec<MemoryStripeGroupSection>,
}

/// Contiguous placement is the compatibility baseline; striped is opt-in.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AddressMappingMode {
    Contiguous,
    Striped,
}

/// Ordered members of one capacity-preserving stripe group.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MemoryStripeGroupSection {
    pub name: String,
    pub devices: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ProcessingElementSection {
    pub name: String,
    pub memory_map: String,
    /// Optional memory map for a connected independent `.dma` issuer.
    /// Omission makes DMA use the compute memory map.
    #[serde(default, deserialize_with = "parse_optional_map_name")]
    pub dma_memory_map: Option<String>,
    pub config: ProcessingElementConfigSection,
}

fn parse_optional_map_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<yaml_serde::Value>::deserialize(deserializer)?;
    value
        .map(|value| match value {
            yaml_serde::Value::String(name) => Ok(name),
            _ => Err(serde::de::Error::custom("DMA memory map must be a string")),
        })
        .transpose()
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessingElementConfigSection {
    pub num_active_requests: Option<usize>,
    pub lsu_access_bytes: Option<usize>,
    pub overhead_size_bytes: Option<usize>,
    #[serde(default, deserialize_with = "parse_optional_u64_byte_str")]
    pub sram_bytes: Option<u64>,
    #[serde(default)]
    pub boundary_payload_service: Option<BoundaryPayloadServiceSection>,
    /// Caller logical local-temporary bytes/actual 1 ns tick, not vendor SRAM.
    #[serde(default)]
    pub local_payload_service: Option<LocalPayloadServiceSection>,
    /// Optional PE-wide bounded block execution limits.
    #[serde(default)]
    pub block_execution:
        Option<gwr_models::processing_element::block_execution::BlockExecutionConfig>,
    pub compute: Option<ComputeSelection>,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalPayloadServiceSection {
    pub bytes_per_tick: u64,
}

/// Explicit caller combined-direction boundary payload, not local hardware
/// traffic.
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoundaryPayloadServiceSection {
    pub bytes_per_tick: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum ComputeSelection {
    Named(String),
    Inline(ComputeConfig),
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CacheSection {
    pub name: String,
    pub coherency_managers: Option<Vec<String>>,
    pub config: CacheConfigSection,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CacheConfigSection {
    pub bw_bytes_per_tick: Option<usize>,
    pub line_size_bytes: Option<usize>,
    /// Maximum tracked original read requests (parents); default 64.
    pub max_inflight_parents: Option<usize>,
    /// Maximum downstream line reads awaiting responses (fills); default 64.
    pub max_inflight_fills: Option<usize>,
    pub num_ways: Option<usize>,
    pub num_sets: Option<usize>,
    pub delay_ticks: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct FabricSection {
    pub name: String,
    pub kind: FabricKind,
    pub columns: usize,
    pub rows: usize,
    pub config: FabricConfigSection,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FabricConfigSection {
    pub fabric_ports_per_node: Option<usize>,
    pub ticks_per_hop: Option<usize>,
    pub ticks_overhead: Option<usize>,
    pub rx_buffer_bytes: Option<usize>,
    pub tx_buffer_bytes: Option<usize>,
    pub port_bits_per_tick: Option<usize>,
    /// Optional independent directional interior service; Routed fabrics only.
    pub internal_link_bits_per_tick: Option<usize>,
    pub routing: Option<FabricRoutingAlgorithm>,
    pub port_selection: Option<FabricPortSelection>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MemorySection {
    pub name: String,
    pub kind: MemoryKind,
    #[serde(deserialize_with = "parse_u64_byte_str")]
    pub base_address: u64,
    pub config: MemoryConfigSection,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfigSection {
    #[serde(deserialize_with = "parse_u64_byte_str")]
    pub capacity_bytes: u64,
    pub bw_bytes_per_tick: Option<usize>,
    pub delay_ticks: Option<usize>,
    /// Optional analytic FIFO byte service; omission preserves legacy timing.
    pub arrival_aware_service: Option<MemoryServiceSection>,
    /// Explicit caller atomic capabilities sharing that exact service budget.
    #[serde(default)]
    pub atomic_service: Option<gwr_models::memory::atomic::AtomicServiceConfig>,
}

/// One total admission bound including queued, active and undelivered
/// responses. Uses configured integer bytes/tick and requires the actual
/// native1ns clock.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryServiceSection {
    /// Nonphysical descriptor limit1..=4096; not a hardware queue-depth claim.
    pub max_pending_requests: usize,
    /// Explicit caller-assumed address service lanes; absent preserves recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<MemoryServiceLanesSection>,
}

/// Caller-assumed relative modulo stripes and integral native1ns rates.
/// This does not certify physical HBM channels, mapping or per-stack rates.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryServiceLanesSection {
    pub interleave_bytes: u64,
    pub bytes_per_tick: Vec<u64>,
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

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CoherencyManagerSection {
    pub name: String,
    pub memory_map: String,
    pub config: CoherencyManagerConfigSection,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CoherencyManagerConfigSection {
    pub line_size_bytes: Option<usize>,
    /// Positive directory work budget per tick; omitted means unlimited.
    pub operations_per_tick: Option<usize>,
}
