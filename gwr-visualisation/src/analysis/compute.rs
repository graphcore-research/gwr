// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};

use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::operators::{Tensor, TensorView};
use gwr_models::processing_element::task::ComputeOp;
use gwr_timetable::timetable_file::{
    NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
};

use super::graph::{is_data_edge, layer_name};
use super::model::{LayerPeSummary, LayerSummary, MachineOpSummary, PeSummary, TensorSummary};
use super::{TensorViewSlots, TimetableIndex, tensor_view_bytes};

#[derive(Default)]
struct LayerBuilder {
    compute_nodes: usize,
    machine_ops: MachineOpSummary,
    tensor_read_bytes: u64,
    tensor_write_bytes: u64,
    by_op: BTreeMap<String, usize>,
    pes: BTreeMap<String, LayerPeSummary>,
    tensors: BTreeSet<String>,
    pe_tensors: BTreeMap<String, BTreeSet<String>>,
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

pub(super) fn apply_compute_allocations(
    pes_by_name: &mut BTreeMap<String, PeSummary>,
    index: &TimetableIndex,
    node_layers: &BTreeMap<String, usize>,
    node_machine_ops: &BTreeMap<String, MachineOpCounts>,
) {
    for (node_id, layer) in node_layers {
        let layer_name = layer_name(*layer);
        let pe_name = index
            .node_pes
            .get(node_id)
            .and_then(Clone::clone)
            .unwrap_or_else(|| "unassigned".to_string());
        let Some(pe) = pes_by_name.get_mut(&pe_name) else {
            continue;
        };

        *pe.by_layer.entry(layer_name.clone()).or_default() += 1;
        if let Some(counts) = node_machine_ops.get(node_id).copied() {
            pe.machine_ops.add_counts(counts);
            pe.machine_ops_by_layer
                .entry(layer_name)
                .or_default()
                .add_counts(counts);
        }
    }
}

pub(super) fn summarize_layers(
    timetable: &TimetableFile,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    index: &TimetableIndex,
    node_layers: &BTreeMap<String, usize>,
    node_machine_ops: &BTreeMap<String, MachineOpCounts>,
) -> Vec<LayerSummary> {
    let mut builders = BTreeMap::new();
    add_compute_nodes(
        &mut builders,
        timetable,
        index,
        node_layers,
        node_machine_ops,
    );
    add_tensor_traffic(&mut builders, timetable, tensors_by_id, index, node_layers);
    build_layer_summaries(builders)
}

fn add_compute_nodes(
    builders: &mut BTreeMap<usize, LayerBuilder>,
    timetable: &TimetableFile,
    index: &TimetableIndex,
    node_layers: &BTreeMap<String, usize>,
    node_machine_ops: &BTreeMap<String, MachineOpCounts>,
) {
    for node in &timetable.nodes {
        let NodeSection::Compute { id, op, .. } = node else {
            continue;
        };
        let Some(layer) = node_layers.get(id).copied() else {
            continue;
        };
        let pe_name = pe_name(index, id);
        let builder = builders.entry(layer).or_default();
        let op_name = op.trace_name().to_string();
        builder.compute_nodes += 1;
        *builder.by_op.entry(op_name.clone()).or_default() += 1;

        let pe = builder.pe_mut(&pe_name);
        pe.compute_nodes += 1;
        *pe.by_op.entry(op_name).or_default() += 1;
        if let Some(counts) = node_machine_ops.get(id).copied() {
            builder.machine_ops.add_counts(counts);
            builder.pe_mut(&pe_name).machine_ops.add_counts(counts);
        }
    }
}

fn add_tensor_traffic(
    builders: &mut BTreeMap<usize, LayerBuilder>,
    timetable: &TimetableFile,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    index: &TimetableIndex,
    node_layers: &BTreeMap<String, usize>,
) {
    let mut slots = TensorViewSlots::default();
    for edge in &timetable.edges {
        if !is_data_edge(edge) {
            continue;
        }
        let from = edge.from_node_id();
        let to = edge.to_node_id();
        let input_index = slots.assign_input(
            to,
            edge.to_node_and_edge().ok().and_then(|(_, index)| index),
            index,
        );
        let output_index = slots.assign_output(
            from,
            edge.from_node_and_edge().ok().and_then(|(_, index)| index),
            index,
        );
        if tensors_by_id.contains_key(from) {
            add_tensor_read(
                builders,
                from,
                to,
                tensors_by_id,
                index,
                node_layers,
                input_index,
            );
        }
        if tensors_by_id.contains_key(to) {
            add_tensor_write(
                builders,
                to,
                from,
                tensors_by_id,
                index,
                node_layers,
                output_index,
            );
        }
    }
}

fn add_tensor_read(
    builders: &mut BTreeMap<usize, LayerBuilder>,
    tensor_id: &str,
    compute_id: &str,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    index: &TimetableIndex,
    node_layers: &BTreeMap<String, usize>,
    input_index: Option<usize>,
) {
    let Some(layer) = node_layers.get(compute_id).copied() else {
        return;
    };
    let bytes = tensor_view_bytes(
        tensor_id,
        compute_id,
        input_index,
        tensors_by_id,
        &index.tensor_configs,
        &index.node_input_views,
    )
    .unwrap_or(0);
    let pe_name = pe_name(index, compute_id);
    let builder = builders.entry(layer).or_default();
    builder.tensor_read_bytes = builder.tensor_read_bytes.saturating_add(bytes);
    record_tensor(builder, &pe_name, tensor_id);
    let pe = builder.pe_mut(&pe_name);
    pe.tensor_read_bytes = pe.tensor_read_bytes.saturating_add(bytes);
}

fn add_tensor_write(
    builders: &mut BTreeMap<usize, LayerBuilder>,
    tensor_id: &str,
    compute_id: &str,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    index: &TimetableIndex,
    node_layers: &BTreeMap<String, usize>,
    output_index: Option<usize>,
) {
    let Some(layer) = node_layers.get(compute_id).copied() else {
        return;
    };
    let bytes = tensor_view_bytes(
        tensor_id,
        compute_id,
        output_index,
        tensors_by_id,
        &index.tensor_configs,
        &index.node_output_views,
    )
    .unwrap_or(0);
    let pe_name = pe_name(index, compute_id);
    let builder = builders.entry(layer).or_default();
    builder.tensor_write_bytes = builder.tensor_write_bytes.saturating_add(bytes);
    record_tensor(builder, &pe_name, tensor_id);
    let pe = builder.pe_mut(&pe_name);
    pe.tensor_write_bytes = pe.tensor_write_bytes.saturating_add(bytes);
}

fn pe_name(index: &TimetableIndex, node_id: &str) -> String {
    index
        .node_pes
        .get(node_id)
        .and_then(Clone::clone)
        .unwrap_or_else(|| "unassigned".to_string())
}

fn record_tensor(builder: &mut LayerBuilder, pe_name: &str, tensor_id: &str) {
    builder.tensors.insert(tensor_id.to_string());
    builder
        .pe_tensors
        .entry(pe_name.to_string())
        .or_default()
        .insert(tensor_id.to_string());
}

fn build_layer_summaries(builders: BTreeMap<usize, LayerBuilder>) -> Vec<LayerSummary> {
    builders
        .into_iter()
        .map(|(layer, builder)| {
            let mut pes = builder.pes;
            for pe in pes.values_mut() {
                pe.tensor_count = builder.pe_tensors.get(&pe.name).map_or(0, BTreeSet::len);
            }
            LayerSummary {
                name: layer_name(layer),
                compute_nodes: builder.compute_nodes,
                machine_ops: builder.machine_ops,
                tensor_count: builder.tensors.len(),
                tensor_read_bytes: builder.tensor_read_bytes,
                tensor_write_bytes: builder.tensor_write_bytes,
                by_op: builder.by_op,
                pes: pes.into_values().collect(),
            }
        })
        .collect()
}

pub(super) fn compute_node_machine_ops(
    timetable: &TimetableFile,
    tensor_configs: &BTreeMap<String, TensorConfigSection>,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, MachineOpCounts> {
    struct ComputeViews<'a> {
        op: &'a ComputeOp,
        input_views: &'a [Option<TensorViewSection>],
        output_views: &'a [Option<TensorViewSection>],
        inputs: Vec<Option<String>>,
        outputs: Vec<Option<String>>,
    }

    let mut compute_views = BTreeMap::new();
    for node in &timetable.nodes {
        if let NodeSection::Compute {
            id,
            op,
            input_views,
            output_views,
            ..
        } = node
        {
            compute_views.insert(
                id.clone(),
                ComputeViews {
                    op,
                    input_views,
                    output_views,
                    inputs: vec![None; input_views.len()],
                    outputs: vec![None; output_views.len()],
                },
            );
        }
    }

    for edge in &timetable.edges {
        if !is_data_edge(edge) {
            continue;
        }
        let from = edge.from_node_id();
        let to = edge.to_node_id();
        if tensor_configs.contains_key(from)
            && let Some(compute) = compute_views.get_mut(to)
        {
            let index = edge.to_node_and_edge().ok().and_then(|(_, index)| index);
            assign_tensor_slot(&mut compute.inputs, index, from);
        }
        if tensor_configs.contains_key(to)
            && let Some(compute) = compute_views.get_mut(from)
        {
            let index = edge.from_node_and_edge().ok().and_then(|(_, index)| index);
            assign_tensor_slot(&mut compute.outputs, index, to);
        }
    }

    compute_views
        .into_iter()
        .filter_map(|(id, compute)| {
            if compute.inputs.is_empty()
                && compute.outputs.is_empty()
                && !matches!(compute.op, ComputeOp::Custom(_))
            {
                return None;
            }
            let inputs = make_tensor_views(&compute.inputs, compute.input_views, tensor_configs);
            let outputs = make_tensor_views(&compute.outputs, compute.output_views, tensor_configs);
            match compute.op.compute_machine_ops(&inputs, &outputs) {
                Ok(counts) => Some((id, counts)),
                Err(error) => {
                    warnings.push(format!(
                        "Unable to calculate static machine ops for '{id}': {error}"
                    ));
                    None
                }
            }
        })
        .collect()
}

fn assign_tensor_slot(slots: &mut [Option<String>], index: Option<usize>, tensor_id: &str) {
    let target = index.or_else(|| slots.iter().position(Option::is_none));
    if let Some(slot) = target.and_then(|index| slots.get_mut(index)) {
        *slot = Some(tensor_id.to_string());
    }
}

fn make_tensor_views(
    tensor_ids: &[Option<String>],
    views: &[Option<TensorViewSection>],
    tensor_configs: &BTreeMap<String, TensorConfigSection>,
) -> Vec<Option<TensorView>> {
    tensor_ids
        .iter()
        .zip(views)
        .map(|(tensor_id, view)| {
            let config = tensor_configs.get(tensor_id.as_deref()?)?;
            let tensor = Tensor::new(&config.shape, &config.dtype, config.addr);
            Some(match view {
                Some(view) => TensorView::new(tensor, &view.shape, &view.offsets),
                None => TensorView::new_full(tensor),
            })
        })
        .collect()
}
