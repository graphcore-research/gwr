// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

#[cfg(feature = "generator")]
use gwr_engine::types::SimError;
#[cfg(feature = "generator")]
use gwr_models::processing_element::MachineOpCounts;
#[cfg(feature = "generator")]
use gwr_models::processing_element::operators::TensorViewLayout;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ReportData {
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
    pub(crate) nodes: u64,
    pub(crate) compute_nodes: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) total_machine_ops: u64,
    pub(crate) tensor_nodes: u64,
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
    pub(crate) data_edges: u64,
    pub(crate) active_pes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PeSummary {
    pub(crate) name: String,
    pub(crate) row: u64,
    pub(crate) col: u64,
    pub(crate) total_nodes: u64,
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
    pub(crate) by_layer: BTreeMap<String, u64>,
    pub(crate) by_op: BTreeMap<String, u64>,
    pub(crate) present_in_timetable: bool,
    pub(crate) present_in_platform: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) platform_config: Option<PePlatformConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) overlays: BTreeMap<String, f64>,
}

impl PeSummary {
    #[cfg(feature = "generator")]
    pub(crate) fn new(name: String, col: u64, row: u64) -> Self {
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
    pub(crate) fn add_counts(
        &mut self,
        counts: MachineOpCounts,
        description: &str,
    ) -> Result<(), SimError> {
        self.adds = checked_count(self.adds, counts.adds, description, "add")?;
        self.muls = checked_count(self.muls, counts.muls, description, "multiply")?;
        self.compares = checked_count(self.compares, counts.compares, description, "comparison")?;
        self.total = self
            .adds
            .checked_add(self.muls)
            .and_then(|total| total.checked_add(self.compares))
            .ok_or_else(|| SimError(format!("{description}: machine operation count overflows")))?;
        Ok(())
    }
}

#[cfg(feature = "generator")]
fn checked_count(
    total: u64,
    count: usize,
    description: &str,
    operation: &str,
) -> Result<u64, SimError> {
    let count = u64::try_from(count).map_err(|error| {
        SimError(format!(
            "{description}: machine {operation} count cannot be represented: {error}"
        ))
    })?;
    total.checked_add(count).ok_or_else(|| {
        SimError(format!(
            "{description}: machine {operation} count overflows"
        ))
    })
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LayerSummary {
    pub(crate) name: String,
    pub(crate) compute_nodes: u64,
    pub(crate) machine_ops: MachineOpSummary,
    pub(crate) tensor_count: u64,
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
    pub(crate) by_op: BTreeMap<String, u64>,
    pub(crate) pes: Vec<LayerPeSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LayerPeSummary {
    pub(crate) name: String,
    pub(crate) compute_nodes: u64,
    pub(crate) machine_ops: MachineOpSummary,
    pub(crate) by_op: BTreeMap<String, u64>,
    pub(crate) tensor_count: u64,
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
        serialize_with = "serialize_optional_u128_as_string",
        deserialize_with = "deserialize_optional_u128"
    )]
    pub(crate) max_addr: Option<u128>,
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
    pub(crate) tensor_count: u64,
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
    #[serde(
        serialize_with = "serialize_u64_slice_as_strings",
        deserialize_with = "deserialize_u64_vec"
    )]
    pub(crate) shape: Vec<u64>,
    pub(crate) writes_by_pe: Vec<TensorPeTraffic>,
    pub(crate) reads_by_pe: Vec<TensorPeTraffic>,
}

fn serialize_u64_as_string<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

fn serialize_u64_slice_as_strings<S>(values: &[u64], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .serialize(serializer)
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

#[allow(clippy::ref_option)]
fn serialize_optional_u128_as_string<S>(
    value: &Option<u128>,
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

#[derive(Deserialize)]
#[serde(untagged)]
enum SerializedU128 {
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

fn deserialize_u64_vec<'de, D>(deserializer: D) -> Result<Vec<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Vec::<SerializedU64>::deserialize(deserializer)?
        .into_iter()
        .map(parse_serialized_u64)
        .collect()
}

fn deserialize_optional_u128<'de, D>(deserializer: D) -> Result<Option<u128>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<SerializedU128>::deserialize(deserializer)?
        .map(|value| match value {
            SerializedU128::Number(value) => Ok(u128::from(value)),
            SerializedU128::String(value) => value.parse().map_err(serde::de::Error::custom),
        })
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
pub(crate) struct TensorPeTraffic {
    pub(crate) pe: String,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) bytes: u64,
    pub(crate) edge_count: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) by_layer: BTreeMap<String, TensorLayerTraffic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) transfers: Vec<TensorTransfer>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TensorTransfer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) layer: Option<String>,
    pub(crate) access: TensorAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TensorAccess {
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) first_element: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) elements_per_range: u64,
    pub(crate) strides: Vec<TensorStride>,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) bits_per_element: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) num_access_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TensorStride {
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) count: u64,
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) stride_elements: u64,
}

#[cfg(feature = "generator")]
impl TryFrom<&TensorViewLayout> for TensorAccess {
    type Error = SimError;

    fn try_from(layout: &TensorViewLayout) -> Result<Self, Self::Error> {
        Ok(Self {
            first_element: report_u64(layout.first_element(), "tensor-view first element")?,
            elements_per_range: report_u64(
                layout.elements_per_range(),
                "tensor-view elements per range",
            )?,
            strides: layout
                .strides()
                .iter()
                .map(|stride| {
                    Ok(TensorStride {
                        count: report_u64(stride.count(), "tensor-view stride count")?,
                        stride_elements: report_u64(
                            stride.stride_elements(),
                            "tensor-view element stride",
                        )?,
                    })
                })
                .collect::<Result<_, SimError>>()?,
            bits_per_element: report_u64(
                layout.bits_per_element(),
                "tensor-view bits per element",
            )?,
            num_access_bytes: report_u64(
                layout.num_access_bytes(),
                "tensor-view access byte count",
            )?,
        })
    }
}

#[cfg(feature = "generator")]
fn report_u64(value: usize, description: &str) -> Result<u64, SimError> {
    u64::try_from(value)
        .map_err(|error| SimError(format!("{description} cannot be represented: {error}")))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TensorLayerTraffic {
    #[serde(
        serialize_with = "serialize_u64_as_string",
        deserialize_with = "deserialize_u64"
    )]
    pub(crate) bytes: u64,
    pub(crate) edge_count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PlatformSummary {
    pub(crate) processing_elements: u64,
    pub(crate) rows: u64,
    pub(crate) cols: u64,
    pub(crate) fabrics: Vec<FabricSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct FabricSummary {
    pub(crate) name: String,
    pub(crate) rows: u64,
    pub(crate) cols: u64,
    pub(crate) kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PePlatformConfig {
    pub(crate) memory_map: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) num_active_requests: Option<u64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_u64_as_string",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub(crate) lsu_access_bytes: Option<u64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_u64_as_string",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub(crate) overhead_size_bytes: Option<u64>,
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

    use super::{PePlatformConfig, TensorSummary};

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

    #[test]
    fn round_trips_tensor_shapes_larger_than_wasm_usize() {
        let tensor = TensorSummary {
            id: "large".into(),
            addr: 0,
            num_bytes: 1,
            dtype: "int8".into(),
            shape: vec![u64::from(u32::MAX) + 1],
            writes_by_pe: Vec::new(),
            reads_by_pe: Vec::new(),
        };

        let json = serde_json::to_value(&tensor).unwrap();
        assert_eq!(json["shape"][0], (u64::from(u32::MAX) + 1).to_string());

        let decoded: TensorSummary = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.shape, tensor.shape);
    }

    #[test]
    fn serializes_processing_element_byte_sizes_as_strings() {
        let config = PePlatformConfig {
            memory_map: "memory-map".into(),
            num_active_requests: Some(8),
            lsu_access_bytes: Some(32),
            overhead_size_bytes: Some(16),
            sram_bytes: Some(1 << 20),
            adds_per_tick: None,
            muls_per_tick: None,
            compares_per_tick: None,
        };

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["num_active_requests"], 8);
        assert_eq!(json["lsu_access_bytes"], "32");
        assert_eq!(json["overhead_size_bytes"], "16");
        assert_eq!(json["sram_bytes"], "1048576");

        let decoded: PePlatformConfig = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.lsu_access_bytes, config.lsu_access_bytes);
        assert_eq!(decoded.overhead_size_bytes, config.overhead_size_bytes);
        assert_eq!(decoded.sram_bytes, config.sram_bytes);
    }
}
