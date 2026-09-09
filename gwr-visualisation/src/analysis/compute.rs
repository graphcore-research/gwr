// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};

use gwr_engine::types::SimError;
use gwr_models::processing_element::MachineOpCounts;
use gwr_timetable::{ComputeTensorDirection, TimetableGraph};

use super::graph::layer_name;
use super::{PeTable, add_map_count, add_u64, pe_index_for_node, u64_from_usize};
use crate::model::{LayerPeSummary, LayerSummary, MachineOpSummary};

pub(super) fn compute_node_machine_ops(
    graph: &TimetableGraph,
) -> Result<Vec<Option<MachineOpCounts>>, SimError> {
    graph
        .nodes()
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let Some(operation) = node.operation() else {
                return Ok(None);
            };
            let views = graph.compute_views(index).ok_or_else(|| {
                SimError(format!(
                    "Compute node '{}' has no resolved tensor views",
                    node.id()
                ))
            })?;
            operation
                .compute_machine_ops(views.inputs(), views.outputs())
                .map(Some)
                .map_err(|error| {
                    SimError(format!(
                        "Unable to calculate machine operations for compute node '{}': {error}",
                        node.id()
                    ))
                })
        })
        .collect()
}

pub(super) fn apply_compute_allocations(
    graph: &TimetableGraph,
    pes: &mut PeTable,
    node_pe_indices: &[Option<usize>],
    node_layers: &[usize],
    node_machine_ops: &[Option<MachineOpCounts>],
) -> Result<(), SimError> {
    for (index, node) in graph.nodes().iter().enumerate() {
        if node.operation().is_none() {
            continue;
        }
        let pe_index = pe_index_for_node(graph, node_pe_indices, index)?;
        let pe = pes.get_mut(pe_index);
        let pe_name = pe.name.clone();
        let layer = layer_name(node_layers[index]);
        add_map_count(&mut pe.by_layer, &layer, "PE layer node count")?;
        if let Some(counts) = node_machine_ops[index] {
            pe.machine_ops
                .add_counts(counts, &format!("PE '{pe_name}'"))?;
            pe.machine_ops_by_layer
                .entry(layer)
                .or_default()
                .add_counts(counts, &format!("PE '{pe_name}' layer total"))?;
        }
    }
    Ok(())
}

pub(super) fn summarize_layers(
    graph: &TimetableGraph,
    pes: &PeTable,
    node_pe_indices: &[Option<usize>],
    node_layers: &[usize],
    node_machine_ops: &[Option<MachineOpCounts>],
) -> Result<Vec<LayerSummary>, SimError> {
    let mut builders: BTreeMap<usize, LayerBuilder> = BTreeMap::new();
    for (index, node) in graph.nodes().iter().enumerate() {
        let Some(operation) = node.operation() else {
            continue;
        };
        let pe_index = pe_index_for_node(graph, node_pe_indices, index)?;
        let pe_name = &pes.get(pe_index).name;
        let builder = builders.entry(node_layers[index]).or_default();
        let operation_name = operation.trace_name();
        add_u64(&mut builder.compute_nodes, 1, "Layer compute-node count")?;
        add_map_count(&mut builder.by_op, operation_name, "Layer operation count")?;

        let pe = builder.pe_mut(pe_name);
        add_u64(&mut pe.compute_nodes, 1, "Layer PE compute-node count")?;
        add_map_count(&mut pe.by_op, operation_name, "Layer PE operation count")?;
        if let Some(counts) = node_machine_ops[index] {
            builder
                .machine_ops
                .add_counts(counts, &format!("Layer {}", node_layers[index]))?;
            builder.pe_mut(pe_name).machine_ops.add_counts(
                counts,
                &format!("Layer {} PE '{pe_name}'", node_layers[index]),
            )?;
        }
    }

    for connection in graph
        .edges()
        .iter()
        .filter_map(|edge| edge.tensor_connection())
    {
        let compute = connection.compute_node();
        let pe_index = pe_index_for_node(graph, node_pe_indices, compute)?;
        let pe_name = &pes.get(pe_index).name;
        let bytes = u64_from_usize(
            connection.view().layout().num_access_bytes(),
            "tensor transfer byte count",
        )?;
        let builder = builders.entry(node_layers[compute]).or_default();
        builder.tensors.insert(connection.tensor_node());
        builder
            .pe_tensors
            .entry(pe_name.to_string())
            .or_default()
            .insert(connection.tensor_node());
        if connection.direction() == ComputeTensorDirection::Input {
            add_u64(
                &mut builder.tensor_read_bytes,
                bytes,
                "Layer tensor read byte total",
            )?;
            add_u64(
                &mut builder.pe_mut(pe_name).tensor_read_bytes,
                bytes,
                "Layer PE tensor read byte total",
            )?;
        } else {
            add_u64(
                &mut builder.tensor_write_bytes,
                bytes,
                "Layer tensor write byte total",
            )?;
            add_u64(
                &mut builder.pe_mut(pe_name).tensor_write_bytes,
                bytes,
                "Layer PE tensor write byte total",
            )?;
        }
    }

    builders
        .into_iter()
        .map(|(layer, mut builder)| {
            for pe in builder.pes.values_mut() {
                pe.tensor_count = builder.pe_tensors.get(&pe.name).map_or(Ok(0), |tensors| {
                    u64_from_usize(tensors.len(), "Layer PE tensor count")
                })?;
            }
            Ok(LayerSummary {
                name: layer_name(layer),
                compute_nodes: builder.compute_nodes,
                machine_ops: builder.machine_ops,
                tensor_count: u64_from_usize(builder.tensors.len(), "Layer tensor count")?,
                tensor_read_bytes: builder.tensor_read_bytes,
                tensor_write_bytes: builder.tensor_write_bytes,
                by_op: builder.by_op,
                pes: builder.pes.into_values().collect(),
            })
        })
        .collect()
}

#[derive(Default)]
struct LayerBuilder {
    compute_nodes: u64,
    machine_ops: MachineOpSummary,
    tensor_read_bytes: u64,
    tensor_write_bytes: u64,
    by_op: BTreeMap<String, u64>,
    pes: BTreeMap<String, LayerPeSummary>,
    tensors: BTreeSet<usize>,
    pe_tensors: BTreeMap<String, BTreeSet<usize>>,
}

impl LayerBuilder {
    fn pe_mut(&mut self, name: &str) -> &mut LayerPeSummary {
        self.pes
            .entry(name.to_string())
            .or_insert_with(|| LayerPeSummary {
                name: name.to_string(),
                compute_nodes: 0,
                machine_ops: MachineOpSummary::default(),
                by_op: BTreeMap::new(),
                tensor_count: 0,
                tensor_read_bytes: 0,
                tensor_write_bytes: 0,
            })
    }
}
