// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::MachineOpCounts;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub(crate) struct VisualisationData {
    pub(super) summary: Summary,
    pub(super) layers: Vec<LayerSummary>,
    pub(super) ops: Vec<String>,
    pub(super) machine_ops: Vec<MachineOpMetadata>,
    pub(super) memory: MemorySummary,
    pub(super) tensors: Vec<TensorSummary>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) overlay_metrics: BTreeMap<String, OverlayMetricMetadata>,
    pub(super) pes: Vec<PeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) platform: Option<PlatformSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct MachineOpMetadata {
    pub(super) name: String,
    pub(super) label: String,
}

pub(super) fn machine_op_metadata() -> Vec<MachineOpMetadata> {
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

#[derive(Debug, Serialize)]
pub(super) struct Summary {
    pub(super) timetable: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) overlay: Option<String>,
    pub(super) nodes: usize,
    pub(super) compute_nodes: usize,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) total_machine_ops: u64,
    pub(super) tensor_nodes: usize,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) total_tensor_read_bytes: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) total_tensor_write_bytes: u64,
    pub(super) memory_nodes: usize,
    pub(super) edges: usize,
    pub(super) active_pes: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct PeSummary {
    pub(super) name: String,
    pub(super) row: usize,
    pub(super) col: usize,
    pub(super) total_nodes: usize,
    pub(super) machine_ops: MachineOpSummary,
    pub(super) machine_ops_by_layer: BTreeMap<String, MachineOpSummary>,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) tensor_read_bytes: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) tensor_write_bytes: u64,
    pub(super) by_layer: BTreeMap<String, usize>,
    pub(super) by_op: BTreeMap<String, usize>,
    pub(super) present_in_timetable: bool,
    pub(super) present_in_platform: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) platform_config: Option<PePlatformConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) overlays: BTreeMap<String, f64>,
}

impl PeSummary {
    pub(super) fn new(name: String, col: usize, row: usize) -> Self {
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

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub(super) struct MachineOpSummary {
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) total: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) adds: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) muls: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) compares: u64,
}

impl MachineOpSummary {
    pub(super) fn add_counts(&mut self, counts: MachineOpCounts) {
        self.adds = self.adds.saturating_add(counts.adds as u64);
        self.muls = self.muls.saturating_add(counts.muls as u64);
        self.compares = self.compares.saturating_add(counts.compares as u64);
        self.total = self
            .adds
            .saturating_add(self.muls)
            .saturating_add(self.compares);
    }
}

#[derive(Debug, Serialize)]
pub(super) struct LayerSummary {
    pub(super) name: String,
    pub(super) compute_nodes: usize,
    pub(super) machine_ops: MachineOpSummary,
    pub(super) tensor_count: usize,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) tensor_read_bytes: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) tensor_write_bytes: u64,
    pub(super) by_op: BTreeMap<String, usize>,
    pub(super) pes: Vec<LayerPeSummary>,
}

#[derive(Debug, Serialize)]
pub(super) struct LayerPeSummary {
    pub(super) name: String,
    pub(super) compute_nodes: usize,
    pub(super) machine_ops: MachineOpSummary,
    pub(super) by_op: BTreeMap<String, usize>,
    pub(super) tensor_count: usize,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) tensor_read_bytes: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) tensor_write_bytes: u64,
}

#[derive(Debug, Serialize)]
pub(super) struct MemorySummary {
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_u64_as_string"
    )]
    pub(super) min_addr: Option<u64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_u64_as_string"
    )]
    pub(super) max_addr: Option<u64>,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) total_memory_read_bytes: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) total_memory_write_bytes: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) platform_memories: Vec<MemoryDeviceSummary>,
}

#[derive(Debug, Serialize)]
pub(super) struct MemoryDeviceSummary {
    pub(super) name: String,
    pub(super) kind: String,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) base_addr: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) capacity_bytes: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) allocated_bytes: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) read_bytes: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) write_bytes: u64,
    pub(super) tensor_count: usize,
    pub(super) tensors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct TensorSummary {
    pub(super) id: String,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) addr: u64,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) num_bytes: u64,
    pub(super) dtype: String,
    pub(super) shape: Vec<usize>,
    pub(super) production_by_pe: Vec<TensorPeConsumption>,
    pub(super) consumption_by_pe: Vec<TensorPeConsumption>,
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct TensorPeConsumption {
    pub(super) pe: String,
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) bytes: u64,
    pub(super) edge_count: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) by_layer: BTreeMap<String, TensorLayerTraffic>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub(super) struct TensorLayerTraffic {
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub(super) bytes: u64,
    pub(super) edge_count: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct PlatformSummary {
    pub(super) processing_elements: usize,
    pub(super) rows: usize,
    pub(super) cols: usize,
    pub(super) fabrics: Vec<FabricSummary>,
}

#[derive(Debug, Serialize)]
pub(super) struct FabricSummary {
    pub(super) name: String,
    pub(super) rows: usize,
    pub(super) cols: usize,
    pub(super) kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PePlatformConfig {
    pub(super) memory_map: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) num_active_requests: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) lsu_access_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) overhead_size_bytes: Option<usize>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_u64_as_string"
    )]
    pub(super) sram_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) adds_per_tick: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) muls_per_tick: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) compares_per_tick: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OverlayInput {
    #[serde(default)]
    pub(super) metrics: BTreeMap<String, OverlayMetricMetadata>,
    #[serde(default)]
    pub(super) metrics_by_pe: BTreeMap<String, BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct OverlayMetricMetadata {
    pub(super) label: Option<String>,
    pub(super) unit: Option<String>,
}
