// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Timetable analysis and report-data construction.

mod compute;
mod graph;
mod memory;
mod platform;
mod tensors;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use compute::{apply_compute_allocations, compute_node_machine_ops, summarize_layers};
use graph::compute_graph_layers;
use gwr_engine::types::SimError;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::TimetableGraph;
use memory::summarize_memory;
use platform::{apply_platform, pe_coords, summarize_platform};
use tensors::{build_tensor_summaries, tensor_traffic_totals};

pub(crate) use crate::model::OverlayInput;
use crate::model::{OverlayMetricMetadata, PeSummary, ReportData, Summary, machine_op_metadata};

pub(crate) fn build_report(
    graph: &TimetableGraph,
    timetable_path: &Path,
    platform: Option<(&PlatformConfig, &Path)>,
    overlay: Option<(&OverlayInput, &Path)>,
) -> Result<ReportData, SimError> {
    ReportBuilder {
        graph,
        timetable_path,
        platform,
        overlay,
    }
    .build()
}

struct ReportBuilder<'a> {
    graph: &'a TimetableGraph,
    timetable_path: &'a Path,
    platform: Option<(&'a PlatformConfig, &'a Path)>,
    overlay: Option<(&'a OverlayInput, &'a Path)>,
}

impl ReportBuilder<'_> {
    fn build(self) -> Result<ReportData, SimError> {
        let unassigned = unassigned_pe_name(
            self.graph,
            self.platform.map(|(platform, _)| platform),
            self.overlay.map(|(overlay, _)| overlay),
        );
        let ComputeNodeIndex {
            pe_indices: node_pe_indices,
            mut pes,
            operations: ops,
        } = self.index_compute_nodes(&unassigned)?;
        let node_layers = compute_graph_layers(self.graph)?;
        let node_machine_ops = compute_node_machine_ops(self.graph)?;
        apply_compute_allocations(
            self.graph,
            &mut pes,
            &node_pe_indices,
            &node_layers,
            &node_machine_ops,
        )?;

        let tensors_by_node =
            build_tensor_summaries(self.graph, &node_layers, &node_pe_indices, &mut pes)?;
        let layers = summarize_layers(
            self.graph,
            &pes,
            &node_pe_indices,
            &node_layers,
            &node_machine_ops,
        )?;
        let memory = summarize_memory(
            self.graph,
            &tensors_by_node,
            self.platform.map(|(platform, _)| platform),
        )?;
        let (total_tensor_read_bytes, total_tensor_write_bytes) =
            tensor_traffic_totals(&tensors_by_node)?;
        let mut tensors = tensors_by_node.into_values().collect::<Vec<_>>();
        tensors.sort_by_key(|tensor| (tensor.addr, tensor.id.clone()));

        let platform = self
            .platform
            .map(|(platform, _)| {
                apply_platform(platform, &mut pes)?;
                summarize_platform(platform)
            })
            .transpose()?;

        let mut warnings = Vec::new();
        let overlay_metrics = self.overlay.map_or_else(BTreeMap::new, |(overlay, _)| {
            apply_overlay(overlay, &mut pes, &mut warnings)
        });

        let mut pes = pes.into_values();
        pes.sort_by_key(|pe| (pe.row, pe.col, pe.name.clone()));
        let summary = self.summary(&pes, total_tensor_read_bytes, total_tensor_write_bytes)?;

        Ok(ReportData {
            summary,
            layers,
            ops: ops.into_iter().collect(),
            machine_ops: machine_op_metadata(),
            memory,
            tensors,
            overlay_metrics,
            pes,
            platform,
            warnings,
        })
    }

    fn summary(
        &self,
        pes: &[PeSummary],
        total_tensor_read_bytes: u64,
        total_tensor_write_bytes: u64,
    ) -> Result<Summary, SimError> {
        let mut total_machine_ops = 0;
        for pe in pes {
            add_u64(
                &mut total_machine_ops,
                pe.machine_ops.total,
                "Report machine operation total",
            )?;
        }

        Ok(Summary {
            timetable: self.timetable_path.display().to_string(),
            platform: self.platform.map(|(_, path)| path.display().to_string()),
            overlay: self.overlay.map(|(_, path)| path.display().to_string()),
            nodes: u64_from_usize(self.graph.nodes().len(), "timetable node count")?,
            compute_nodes: u64_from_usize(
                self.graph
                    .nodes()
                    .iter()
                    .filter(|node| node.operation().is_some())
                    .count(),
                "compute-node count",
            )?,
            total_machine_ops,
            tensor_nodes: u64_from_usize(
                self.graph
                    .nodes()
                    .iter()
                    .filter(|node| node.tensor().is_some())
                    .count(),
                "tensor-node count",
            )?,
            total_tensor_read_bytes,
            total_tensor_write_bytes,
            data_edges: u64_from_usize(
                self.graph
                    .edges()
                    .iter()
                    .filter(|edge| edge.tensor_connection().is_some())
                    .count(),
                "data-edge count",
            )?,
            active_pes: u64_from_usize(
                pes.iter().filter(|pe| pe.total_nodes > 0).count(),
                "active PE count",
            )?,
        })
    }

    fn index_compute_nodes(&self, unassigned: &str) -> Result<ComputeNodeIndex, SimError> {
        let mut node_pe_indices = vec![None; self.graph.nodes().len()];
        let mut pes = PeTable::default();
        let mut operations = BTreeSet::new();
        for (index, node) in self.graph.nodes().iter().enumerate() {
            let Some(operation) = node.operation() else {
                continue;
            };
            let pe_name = node.pe().unwrap_or(unassigned).to_string();
            operations.insert(operation.trace_name().to_string());
            let (col, row) = pe_coords(&pe_name).unwrap_or((0, 0));
            let pe_index = pes.get_or_insert(pe_name, col, row);
            node_pe_indices[index] = Some(pe_index);
            let pe = pes.get_mut(pe_index);
            pe.present_in_timetable = true;
            add_u64(&mut pe.total_nodes, 1, "PE compute-node count")?;
            add_map_count(&mut pe.by_op, operation.trace_name(), "PE operation count")?;
        }
        Ok(ComputeNodeIndex {
            pe_indices: node_pe_indices,
            pes,
            operations,
        })
    }
}

struct ComputeNodeIndex {
    pe_indices: Vec<Option<usize>>,
    pes: PeTable,
    operations: BTreeSet<String>,
}

#[derive(Default)]
struct PeTable {
    values: Vec<PeSummary>,
    indices: HashMap<String, usize>,
}

impl PeTable {
    fn get_or_insert(&mut self, name: String, col: u64, row: u64) -> usize {
        if let Some(index) = self.indices.get(&name) {
            return *index;
        }
        let index = self.values.len();
        self.indices.insert(name.clone(), index);
        self.values.push(PeSummary::new(name, col, row));
        index
    }

    fn get(&self, index: usize) -> &PeSummary {
        &self.values[index]
    }

    fn get_mut(&mut self, index: usize) -> &mut PeSummary {
        &mut self.values[index]
    }

    fn get_mut_by_name(&mut self, name: &str) -> Option<&mut PeSummary> {
        let index = *self.indices.get(name)?;
        Some(&mut self.values[index])
    }

    fn into_values(self) -> Vec<PeSummary> {
        self.values
    }
}

fn pe_index_for_node(
    graph: &TimetableGraph,
    node_pe_indices: &[Option<usize>],
    node_index: usize,
) -> Result<usize, SimError> {
    node_pe_indices[node_index].ok_or_else(|| {
        SimError(format!(
            "Compute node '{}' has no report PE",
            graph.nodes()[node_index].id()
        ))
    })
}

fn unassigned_pe_name(
    graph: &TimetableGraph,
    platform: Option<&PlatformConfig>,
    overlay: Option<&OverlayInput>,
) -> String {
    let mut pe_names = BTreeSet::new();
    pe_names.extend(graph.nodes().iter().filter_map(|node| node.pe()));
    pe_names.extend(
        platform
            .and_then(|platform| platform.processing_elements.as_ref())
            .into_iter()
            .flatten()
            .map(|pe| pe.name.as_str()),
    );
    pe_names.extend(
        overlay
            .into_iter()
            .flat_map(|overlay| overlay.metrics_by_pe.keys().map(String::as_str)),
    );

    let mut suffix = 0;
    loop {
        let candidate = if suffix == 0 {
            "unassigned".to_string()
        } else {
            format!("unassigned_{suffix}")
        };
        if !pe_names.contains(candidate.as_str()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn apply_overlay(
    overlay: &OverlayInput,
    pes: &mut PeTable,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, OverlayMetricMetadata> {
    let mut metadata = overlay.metrics.clone();
    for (pe_name, metrics) in &overlay.metrics_by_pe {
        for name in metrics.keys() {
            metadata
                .entry(name.clone())
                .or_insert(OverlayMetricMetadata {
                    label: None,
                    unit: None,
                });
        }
        if let Some(pe) = pes.get_mut_by_name(pe_name) {
            pe.overlays.extend(metrics.clone());
        } else {
            warnings.push(format!("Overlay references unknown PE '{pe_name}'"));
        }
    }
    metadata
}

pub(super) fn add_u64(total: &mut u64, value: u64, description: &str) -> Result<(), SimError> {
    *total = total
        .checked_add(value)
        .ok_or_else(|| SimError(format!("{description} overflows")))?;
    Ok(())
}

pub(super) fn add_map_count(
    counts: &mut BTreeMap<String, u64>,
    name: &str,
    description: &str,
) -> Result<(), SimError> {
    add_u64(counts.entry(name.to_string()).or_default(), 1, description)
}

pub(super) fn u64_from_usize(value: usize, description: &str) -> Result<u64, SimError> {
    u64::try_from(value)
        .map_err(|error| SimError(format!("{description} cannot be represented: {error}")))
}

pub(super) fn u64_from_u128(value: u128, description: &str) -> Result<u64, SimError> {
    u64::try_from(value)
        .map_err(|error| SimError(format!("{description} cannot be represented: {error}")))
}

#[cfg(test)]
mod tests;
