// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_models::processing_element::operators::{Tensor, TensorView};
use gwr_timetable::timetable_file::{EdgeSection, TensorConfigSection, TensorViewSection};

use super::TimetableIndex;
use super::graph::{is_data_edge, layer_name};
use crate::model::{
    PeSummary, TensorLayerTraffic, TensorPeConsumption, TensorSummary, TensorTrafficAccess,
    TensorTrafficRange,
};

pub(super) fn apply_tensor_edges(
    edges: &[EdgeSection],
    tensors_by_id: &mut BTreeMap<String, TensorSummary>,
    index: &TimetableIndex,
    node_layers: &BTreeMap<String, usize>,
) {
    let mut slots = TensorViewSlots::default();
    for edge in edges {
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
        if tensors_by_id.contains_key(from)
            && let Some(pe_name) = index.node_pe_name(to)
        {
            let accesses = tensor_node_accesses(
                from,
                to,
                input_index,
                tensors_by_id,
                index,
                &TensorViewDirection::Input,
            );
            if let Some(tensor) = tensors_by_id.get_mut(from) {
                push_traffic(
                    &mut tensor.consumption_by_pe,
                    pe_name,
                    accesses,
                    node_layers.get(to).copied().map(layer_name).as_deref(),
                );
            }
        }

        if tensors_by_id.contains_key(to)
            && let Some(pe_name) = index.node_pe_name(from)
        {
            let accesses = tensor_node_accesses(
                to,
                from,
                output_index,
                tensors_by_id,
                index,
                &TensorViewDirection::Output,
            );
            if let Some(tensor) = tensors_by_id.get_mut(to) {
                push_traffic(
                    &mut tensor.production_by_pe,
                    pe_name,
                    accesses,
                    node_layers.get(from).copied().map(layer_name).as_deref(),
                );
            }
        }
    }
}

#[derive(Default)]
pub(super) struct TensorViewSlots {
    inputs: BTreeMap<String, Vec<bool>>,
    outputs: BTreeMap<String, Vec<bool>>,
}

impl TensorViewSlots {
    pub(super) fn assign_input(
        &mut self,
        node_id: &str,
        edge_index: Option<usize>,
        index: &TimetableIndex,
    ) -> Option<usize> {
        assign_view_slot(
            &mut self.inputs,
            node_id,
            edge_index,
            &index.node_input_views,
        )
    }

    pub(super) fn assign_output(
        &mut self,
        node_id: &str,
        edge_index: Option<usize>,
        index: &TimetableIndex,
    ) -> Option<usize> {
        assign_view_slot(
            &mut self.outputs,
            node_id,
            edge_index,
            &index.node_output_views,
        )
    }
}

fn assign_view_slot(
    assigned_by_node: &mut BTreeMap<String, Vec<bool>>,
    node_id: &str,
    edge_index: Option<usize>,
    views_by_node: &BTreeMap<String, Vec<Option<TensorViewSection>>>,
) -> Option<usize> {
    let views = views_by_node.get(node_id)?;
    let assigned = assigned_by_node
        .entry(node_id.to_string())
        .or_insert_with(|| vec![false; views.len()]);
    let slot = edge_index.or_else(|| assigned.iter().position(|assigned| !assigned))?;
    if slot >= assigned.len() {
        assigned.resize(slot + 1, false);
    }
    assigned[slot] = true;
    Some(slot)
}

pub(super) fn apply_pe_tensor_traffic(
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    pes_by_name: &mut BTreeMap<String, PeSummary>,
) {
    for tensor in tensors_by_id.values() {
        for consumption in &tensor.consumption_by_pe {
            if let Some(pe) = pes_by_name.get_mut(&consumption.pe) {
                pe.tensor_read_bytes = pe.tensor_read_bytes.saturating_add(consumption.bytes);
            }
        }
        for production in &tensor.production_by_pe {
            if let Some(pe) = pes_by_name.get_mut(&production.pe) {
                pe.tensor_write_bytes = pe.tensor_write_bytes.saturating_add(production.bytes);
            }
        }
    }
}

pub(super) fn summarize_tensor_traffic(
    tensors_by_id: &BTreeMap<String, TensorSummary>,
) -> (u64, u64) {
    let mut read_bytes = 0_u64;
    let mut write_bytes = 0_u64;
    for tensor in tensors_by_id.values() {
        let tensor_read_bytes = tensor
            .consumption_by_pe
            .iter()
            .fold(0_u64, |total, consumption| {
                total.saturating_add(consumption.bytes)
            });
        let tensor_write_bytes = tensor
            .production_by_pe
            .iter()
            .fold(0_u64, |total, production| {
                total.saturating_add(production.bytes)
            });
        read_bytes = read_bytes.saturating_add(tensor_read_bytes);
        write_bytes = write_bytes.saturating_add(tensor_write_bytes);
    }
    (read_bytes, write_bytes)
}

pub(super) fn tensor_view_bytes(
    tensor_id: &str,
    compute_node: &str,
    edge_index: Option<usize>,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    tensor_configs_by_id: &BTreeMap<String, TensorConfigSection>,
    node_views: &BTreeMap<String, Vec<Option<TensorViewSection>>>,
) -> Option<u64> {
    tensors_by_id.get(tensor_id)?;
    let config = tensor_configs_by_id.get(tensor_id)?;
    let views = node_views.get(compute_node)?;
    let view = match edge_index {
        Some(index) => views.get(index)?.as_ref(),
        None => None,
    };
    let view = physical_view(config, view)?;
    u64::try_from(view.num_access_bytes()).ok()
}

enum TensorViewDirection {
    Input,
    Output,
}

fn tensor_node_accesses(
    tensor_id: &str,
    node_id: &str,
    edge_index: Option<usize>,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    index: &TimetableIndex,
    direction: &TensorViewDirection,
) -> Option<Vec<TensorTrafficRange>> {
    let node_views = match direction {
        TensorViewDirection::Input => &index.node_input_views,
        TensorViewDirection::Output => &index.node_output_views,
    };
    compute_view_accesses(
        tensor_id,
        node_id,
        edge_index,
        tensors_by_id,
        &index.tensor_configs,
        node_views,
    )
}

fn compute_view_accesses(
    tensor_id: &str,
    compute_node: &str,
    edge_index: Option<usize>,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    tensor_configs_by_id: &BTreeMap<String, TensorConfigSection>,
    node_views: &BTreeMap<String, Vec<Option<TensorViewSection>>>,
) -> Option<Vec<TensorTrafficRange>> {
    tensors_by_id.get(tensor_id)?;
    let config = tensor_configs_by_id.get(tensor_id)?;
    let views = node_views.get(compute_node)?;
    let view = match edge_index {
        Some(index) => views.get(index)?.as_ref(),
        None => None,
    };
    physical_view(config, view)?
        .byte_ranges()
        .map(|range| absolute_access(config.addr, range))
        .collect()
}

fn physical_view(
    config: &TensorConfigSection,
    view: Option<&TensorViewSection>,
) -> Option<TensorView> {
    let tensor = Tensor::new(&config.shape, &config.dtype, config.addr).ok()?;
    match view {
        Some(view) => TensorView::new(tensor, &view.shape, &view.offsets).ok(),
        None => Some(TensorView::new_full(tensor)),
    }
}

fn absolute_access(
    tensor_addr: u64,
    relative_range: std::ops::Range<usize>,
) -> Option<TensorTrafficRange> {
    let byte_offset = u64::try_from(relative_range.start).ok()?;
    let num_bytes = u64::try_from(relative_range.len()).ok()?;
    let addr = tensor_addr.checked_add(byte_offset)?;
    addr.checked_add(num_bytes.checked_sub(1)?)?;
    Some(TensorTrafficRange { addr, num_bytes })
}

fn push_traffic(
    consumption_by_pe: &mut Vec<TensorPeConsumption>,
    pe: &str,
    ranges: Option<Vec<TensorTrafficRange>>,
    layer: Option<&str>,
) {
    let ranges = ranges.unwrap_or_default();
    let bytes = ranges
        .iter()
        .fold(0_u64, |total, range| total.saturating_add(range.num_bytes));
    let access = TensorTrafficAccess {
        layer: layer.map(str::to_string),
        ranges,
    };
    if let Some(entry) = consumption_by_pe.iter_mut().find(|entry| entry.pe == pe) {
        entry.bytes = entry.bytes.saturating_add(bytes);
        entry.edge_count += 1;
        if let Some(layer) = layer {
            let traffic = entry.by_layer.entry(layer.to_string()).or_default();
            traffic.bytes = traffic.bytes.saturating_add(bytes);
            traffic.edge_count += 1;
        }
        entry.accesses.push(access);
        return;
    }

    let mut by_layer = BTreeMap::new();
    if let Some(layer) = layer {
        by_layer.insert(
            layer.to_string(),
            TensorLayerTraffic {
                bytes,
                edge_count: 1,
            },
        );
    }
    consumption_by_pe.push(TensorPeConsumption {
        pe: pe.to_string(),
        bytes,
        edge_count: 1,
        by_layer,
        accesses: vec![access],
    });
    consumption_by_pe.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.pe.cmp(&b.pe)));
}
