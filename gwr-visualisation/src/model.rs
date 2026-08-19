// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

#[cfg(feature = "generator")]
use gwr_models::processing_element::MachineOpCounts;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct VisualisationData {
    pub(crate) summary: Summary,
    pub(crate) layers: Vec<LayerSummary>,
    pub(crate) ops: Vec<String>,
    pub(crate) machine_ops: Vec<MachineOpMetadata>,
    pub(crate) memory: MemorySummary,
    pub(crate) tensors: Vec<TensorSummary>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) overlay_metrics: BTreeMap<String, OverlayMetricMetadata>,
    pub(crate) pes: Vec<PeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) platform: Option<PlatformSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MachineOpMetadata {
    pub(crate) name: String,
    pub(crate) label: String,
}

#[cfg(feature = "generator")]
pub(crate) fn machine_op_metadata() -> Vec<MachineOpMetadata> {
    [
        ("adds", "Adds"),
        ("compares", "Compares"),
        ("muls", "Multiplies"),
    ]
    .into_iter()
    .map(|(name, label)| MachineOpMetadata {
        name: name.to_string(),
        label: label.to_string(),
    })
    .collect()
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub(crate) timetable: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) overlay: Option<String>,
    pub(crate) nodes: usize,
    pub(crate) compute_nodes: usize,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) total_machine_ops: u64,
    pub(crate) tensor_nodes: usize,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) total_tensor_read_bytes: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) total_tensor_write_bytes: u64,
    pub(crate) memory_nodes: usize,
    pub(crate) data_edges: usize,
    pub(crate) active_pes: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PeSummary {
    pub(crate) name: String,
    pub(crate) row: usize,
    pub(crate) col: usize,
    pub(crate) total_nodes: usize,
    pub(crate) machine_ops: MachineOpSummary,
    pub(crate) machine_ops_by_layer: BTreeMap<String, MachineOpSummary>,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) tensor_read_bytes: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) tensor_write_bytes: u64,
    pub(crate) by_layer: BTreeMap<String, usize>,
    pub(crate) by_op: BTreeMap<String, usize>,
    pub(crate) present_in_timetable: bool,
    pub(crate) present_in_platform: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) platform_config: Option<PePlatformConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) overlays: BTreeMap<String, f64>,
}

impl PeSummary {
    #[cfg(feature = "generator")]
    pub(crate) fn new(name: String, col: usize, row: usize) -> Self {
        Self {
            name,
            row,
            col,
            total_nodes: 0,
            machine_ops: MachineOpSummary::default(),
            machine_ops_by_layer: BTreeMap::new(),
            tensor_read_bytes: 0,
            tensor_write_bytes: 0,
            by_layer: BTreeMap::new(),
            by_op: BTreeMap::new(),
            present_in_timetable: false,
            present_in_platform: false,
            platform_config: None,
            overlays: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct MachineOpSummary {
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) total: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) adds: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) muls: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) compares: u64,
}

impl MachineOpSummary {
    #[cfg(feature = "generator")]
    pub(crate) fn add_counts(&mut self, counts: MachineOpCounts) {
        self.adds = self.adds.saturating_add(counts.adds as u64);
        self.muls = self.muls.saturating_add(counts.muls as u64);
        self.compares = self.compares.saturating_add(counts.compares as u64);
        self.total = self
            .adds
            .saturating_add(self.muls)
            .saturating_add(self.compares);
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LayerSummary {
    pub(crate) name: String,
    pub(crate) compute_nodes: usize,
    pub(crate) machine_ops: MachineOpSummary,
    pub(crate) tensor_count: usize,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) tensor_read_bytes: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) tensor_write_bytes: u64,
    pub(crate) by_op: BTreeMap<String, usize>,
    pub(crate) pes: Vec<LayerPeSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LayerPeSummary {
    pub(crate) name: String,
    pub(crate) compute_nodes: usize,
    pub(crate) machine_ops: MachineOpSummary,
    pub(crate) by_op: BTreeMap<String, usize>,
    pub(crate) tensor_count: usize,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) tensor_read_bytes: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) tensor_write_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MemorySummary {
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_u64_as_string",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub(crate) min_addr: Option<u64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_u64_as_string",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub(crate) max_addr: Option<u64>,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) total_memory_read_bytes: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) total_memory_write_bytes: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) platform_memories: Vec<MemoryDeviceSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MemoryDeviceSummary {
    pub(crate) name: String,
    pub(crate) kind: String,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) base_addr: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) capacity_bytes: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) allocated_bytes: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) read_bytes: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) write_bytes: u64,
    pub(crate) tensor_count: usize,
    pub(crate) tensors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TensorSummary {
    pub(crate) id: String,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) addr: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) num_bytes: u64,
    pub(crate) dtype: String,
    pub(crate) shape: Vec<usize>,
    pub(crate) production_by_pe: Vec<TensorPeConsumption>,
    pub(crate) consumption_by_pe: Vec<TensorPeConsumption>,
}

fn serialize_u64_as_string<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[allow(clippy::ref_option)]
fn serialize_optional_u64_as_string<S>(
    value: &Option<u64>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(&value.to_string()),
        None => serializer.serialize_none(),
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SerializedU64 {
    Number(u64),
    String(String),
}

fn deserialize_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    parse_serialized_u64(SerializedU64::deserialize(deserializer)?)
}

fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<SerializedU64>::deserialize(deserializer)?
        .map(parse_serialized_u64)
        .transpose()
}

fn parse_serialized_u64<E>(value: SerializedU64) -> Result<u64, E>
where
    E: serde::de::Error,
{
    match value {
        SerializedU64::Number(value) => Ok(value),
        SerializedU64::String(value) => value.parse().map_err(E::custom),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TensorPeConsumption {
    pub(crate) pe: String,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) bytes: u64,
    pub(crate) edge_count: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) by_layer: BTreeMap<String, TensorLayerTraffic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) accesses: Vec<TensorTrafficAccess>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TensorTrafficAccess {
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) addr: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) num_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) layer: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TensorLayerTraffic {
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) bytes: u64,
    pub(crate) edge_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PlatformSummary {
    pub(crate) processing_elements: usize,
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) fabrics: Vec<FabricSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct FabricSummary {
    pub(crate) name: String,
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PePlatformConfig {
    pub(crate) memory_map: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) num_active_requests: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lsu_access_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) overhead_size_bytes: Option<usize>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_u64_as_string",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub(crate) sram_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) adds_per_tick: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) muls_per_tick: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) compares_per_tick: Option<f64>,
}

#[cfg(feature = "generator")]
#[derive(Debug, Deserialize)]
pub(crate) struct OverlayInput {
    #[serde(default)]
    pub(crate) metrics: BTreeMap<String, OverlayMetricMetadata>,
    #[serde(default)]
    pub(crate) metrics_by_pe: BTreeMap<String, BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OverlayMetricMetadata {
    pub(crate) label: Option<String>,
    pub(crate) unit: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct RequiredAddress {
        #[serde(deserialize_with = "super::deserialize_u64")]
        value: u64,
    }

    #[derive(Deserialize)]
    struct OptionalAddress {
        #[serde(deserialize_with = "super::deserialize_optional_u64")]
        value: Option<u64>,
    }

    #[test]
    fn parses_addresses_from_strings_and_numbers() {
        let string: RequiredAddress =
            serde_json::from_str(r#"{"value":"18446744073709551615"}"#).unwrap();
        let number: RequiredAddress = serde_json::from_str(r#"{"value":42}"#).unwrap();

        assert_eq!(string.value, u64::MAX);
        assert_eq!(number.value, 42);
    }

    #[test]
    fn parses_optional_addresses() {
        let string: OptionalAddress = serde_json::from_str(r#"{"value":"7"}"#).unwrap();
        let absent: OptionalAddress = serde_json::from_str(r#"{"value":null}"#).unwrap();

        assert_eq!(string.value, Some(7));
        assert_eq!(absent.value, None);
    }

    #[test]
    fn rejects_invalid_address_strings() {
        let error = serde_json::from_str::<RequiredAddress>(r#"{"value":"invalid"}"#).unwrap_err();

        assert!(error.to_string().contains("invalid digit"));
    }
}
