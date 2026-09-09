// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Timetable analysis and report-model construction.

mod compute;
mod graph;
mod memory;
mod model;
mod platform;
mod tensors;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use compute::{apply_compute_allocations, compute_node_machine_ops, summarize_layers};
use graph::{compute_graph_layers, summarize_graph};
use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::task::ComputeOp;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::timetable_file::{
    NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
};
use memory::summarize_memory;
pub(crate) use model::OverlayInput;
use model::{
    ComputeNodeSummary, MachineOpSummary, PeSummary, Summary, TensorSummary, VisualisationData,
    machine_op_metadata,
};
use platform::{apply_platform, pe_coords, summarize_platform};
use tensors::{
    TensorViewSlots, apply_pe_tensor_traffic, apply_tensor_edges, summarize_tensor_traffic,
    tensor_view_bytes,
};

#[derive(Default)]
struct TimetableIndex {
    tensor_configs: BTreeMap<String, TensorConfigSection>,
    node_pes: BTreeMap<String, Option<String>>,
    node_input_views: BTreeMap<String, Vec<Option<TensorViewSection>>>,
    node_output_views: BTreeMap<String, Vec<Option<TensorViewSection>>>,
    node_ops: BTreeMap<String, String>,
}

#[derive(Default)]
struct TimetableCounts {
    compute_nodes: usize,
    tensor_nodes: usize,
    memory_nodes: usize,
}

#[derive(Default)]
struct IndexedTimetable {
    index: TimetableIndex,
    counts: TimetableCounts,
    ops: BTreeSet<String>,
    pes_by_name: BTreeMap<String, PeSummary>,
    tensors_by_id: BTreeMap<String, TensorSummary>,
}

impl IndexedTimetable {
    fn new(timetable: &TimetableFile) -> Self {
        let mut indexed = Self::default();
        for node in &timetable.nodes {
            indexed.add_node(node);
        }
        indexed
    }

    fn add_node(&mut self, node: &NodeSection) {
        match node {
            NodeSection::Compute {
                id,
                op,
                pe,
                input_views,
                output_views,
                ..
            } => self.add_compute_node(id, op, pe.as_ref(), input_views, output_views),
            NodeSection::Tensor { id, config } => self.add_tensor_node(id, config),
        }
    }

    fn add_compute_node(
        &mut self,
        id: &str,
        op: &ComputeOp,
        pe: Option<&String>,
        input_views: &[Option<TensorViewSection>],
        output_views: &[Option<TensorViewSection>],
    ) {
        self.counts.compute_nodes += 1;
        let op_name = op.trace_name().to_string();
        self.index.node_pes.insert(id.to_string(), pe.cloned());
        self.index.node_ops.insert(id.to_string(), op_name.clone());
        self.index
            .node_input_views
            .insert(id.to_string(), input_views.to_vec());
        self.index
            .node_output_views
            .insert(id.to_string(), output_views.to_vec());
        self.ops.insert(op_name.clone());

        let pe_name = pe.cloned().unwrap_or_else(|| "unassigned".to_string());
        let (col, row) = pe_coords(&pe_name).unwrap_or((0, 0));
        let summary = self
            .pes_by_name
            .entry(pe_name.clone())
            .or_insert_with(|| PeSummary::new(pe_name, col, row));
        summary.present_in_timetable = true;
        summary.total_nodes += 1;
        *summary.by_op.entry(op_name).or_default() += 1;
    }

    fn add_tensor_node(&mut self, id: &str, config: &TensorConfigSection) {
        self.counts.tensor_nodes += 1;
        self.index
            .tensor_configs
            .insert(id.to_string(), config.clone());
        self.tensors_by_id.insert(
            id.to_string(),
            TensorSummary {
                id: id.to_string(),
                addr: config.addr,
                num_bytes: tensor_config_bytes(config),
                dtype: format!("{:?}", config.dtype).to_lowercase(),
                element_bits: config.dtype.num_bits(),
                shape: config.shape.clone(),
                views: Vec::new(),
                accesses: Vec::new(),
                production_by_pe: Vec::new(),
                consumption_by_pe: Vec::new(),
            },
        );
    }
}

fn tensor_config_bytes(config: &TensorConfigSection) -> u64 {
    let elements = config
        .shape
        .iter()
        .try_fold(1_u128, |total, dim| total.checked_mul(*dim as u128));
    let Some(elements) = elements else {
        return u64::MAX;
    };
    let bits = elements.saturating_mul(config.dtype.num_bits() as u128);
    u64::try_from(bits.div_ceil(8)).unwrap_or(u64::MAX)
}

#[must_use]
pub(crate) fn summarize(
    timetable: &TimetableFile,
    timetable_path: &Path,
    platform: Option<(&PlatformConfig, &Path)>,
    overlay: Option<(&OverlayInput, &Path)>,
) -> VisualisationData {
    let mut warnings = Vec::new();
    let IndexedTimetable {
        index,
        counts,
        ops,
        mut pes_by_name,
        mut tensors_by_id,
    } = IndexedTimetable::new(timetable);

    let node_layers = compute_graph_layers(timetable);
    let node_machine_ops =
        compute_node_machine_ops(timetable, &index.tensor_configs, &mut warnings);
    let compute_nodes = summarize_compute_nodes(&index, &node_layers, &node_machine_ops);
    let compute_node_indices = compute_nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect();
    let graph = summarize_graph(timetable, &compute_nodes, &compute_node_indices);
    apply_compute_allocations(&mut pes_by_name, &index, &node_layers, &node_machine_ops);

    apply_tensor_edges(
        &timetable.edges,
        &mut tensors_by_id,
        &index,
        &node_layers,
        &compute_node_indices,
    );
    let layer_summaries = summarize_layers(
        timetable,
        &tensors_by_id,
        &index,
        &node_layers,
        &node_machine_ops,
    );
    apply_pe_tensor_traffic(&tensors_by_id, &mut pes_by_name);
    let memory = summarize_memory(&tensors_by_id, platform.map(|(platform, _)| platform));
    let (total_tensor_read_bytes, total_tensor_write_bytes) =
        summarize_tensor_traffic(&tensors_by_id);
    let mut tensors: Vec<_> = tensors_by_id.into_values().collect();
    tensors.sort_by_key(|tensor| (tensor.addr, tensor.id.clone()));

    let platform_summary = platform.map(|(platform, _)| {
        apply_platform(platform, &mut pes_by_name);
        summarize_platform(platform)
    });

    if let Some((overlay, _)) = overlay {
        apply_overlay(overlay, &mut pes_by_name, &mut warnings);
    }

    let mut pes: Vec<_> = pes_by_name.into_values().collect();
    pes.sort_by_key(|pe| (pe.row, pe.col, pe.name.clone()));
    let active_pes = pes.iter().filter(|pe| pe.total_nodes > 0).count();
    let total_machine_ops = pes.iter().fold(0_u64, |total, pe| {
        total.saturating_add(pe.machine_ops.total)
    });

    VisualisationData {
        summary: Summary {
            timetable: timetable_path.display().to_string(),
            platform: platform.map(|(_, path)| path.display().to_string()),
            overlay: overlay.map(|(_, path)| path.display().to_string()),
            nodes: timetable.nodes.len(),
            compute_nodes: counts.compute_nodes,
            total_machine_ops,
            tensor_nodes: counts.tensor_nodes,
            total_tensor_read_bytes,
            total_tensor_write_bytes,
            memory_nodes: counts.memory_nodes,
            edges: timetable.edges.len(),
            active_pes,
        },
        layers: layer_summaries,
        compute_nodes,
        graph,
        ops: ops.into_iter().collect(),
        machine_ops: machine_op_metadata(),
        memory,
        tensors,
        overlay_metrics: overlay
            .map(|(overlay, _)| overlay.metrics.clone())
            .unwrap_or_default(),
        pes,
        platform: platform_summary,
        warnings,
    }
}

fn summarize_compute_nodes(
    index: &TimetableIndex,
    node_layers: &BTreeMap<String, usize>,
    node_machine_ops: &BTreeMap<String, MachineOpCounts>,
) -> Vec<ComputeNodeSummary> {
    index
        .node_ops
        .iter()
        .map(|(id, op)| {
            let counts = node_machine_ops.get(id).copied().unwrap_or_default();
            ComputeNodeSummary {
                id: id.clone(),
                pe: index
                    .node_pes
                    .get(id)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| "unassigned".to_string()),
                layer: node_layers
                    .get(id)
                    .copied()
                    .map(graph::layer_name)
                    .unwrap_or_default(),
                op: op.clone(),
                machine_ops: MachineOpSummary::from_counts(counts).total,
                dominant_machine_op: dominant_machine_op(counts).map(str::to_string),
            }
        })
        .collect()
}

fn dominant_machine_op(counts: MachineOpCounts) -> Option<&'static str> {
    let mut dominant = None;
    for (name, count) in [
        ("adds", counts.adds),
        ("compares", counts.compares),
        ("muls", counts.muls),
    ] {
        if count > dominant.map_or(0, |(_, maximum)| maximum) {
            dominant = Some((name, count));
        }
    }
    dominant.map(|(name, _)| name)
}

fn apply_overlay(
    overlay: &OverlayInput,
    pes_by_name: &mut BTreeMap<String, PeSummary>,
    warnings: &mut Vec<String>,
) {
    for (pe_name, metrics) in &overlay.metrics_by_pe {
        if let Some(pe) = pes_by_name.get_mut(pe_name) {
            pe.overlays.extend(metrics.clone());
        } else {
            warnings.push(format!("Overlay references unknown PE '{pe_name}'"));
        }
    }
}

#[cfg(test)]
mod tests;
