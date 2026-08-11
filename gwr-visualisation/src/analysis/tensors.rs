// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_timetable::timetable_file::{
    EdgeSection, MemoryConfigSection, TensorConfigSection, TensorViewSection,
    checked_dtype_byte_range,
};

use super::TimetableIndex;
use super::graph::{is_data_edge, layer_name};
use super::model::{PeSummary, TensorLayerTraffic, TensorPeConsumption, TensorSummary};

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
        if tensors_by_id.contains_key(from) && index.node_pes.contains_key(to) {
            let bytes = tensor_node_bytes(
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
                    index.node_pes.get(to).and_then(Option::as_deref),
                    bytes,
                    node_layers.get(to).copied().map(layer_name).as_deref(),
                );
            }
        }

        if tensors_by_id.contains_key(to) && index.node_pes.contains_key(from) {
            let bytes = tensor_node_bytes(
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
                    index.node_pes.get(from).and_then(Option::as_deref),
                    bytes,
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
    let tensor = tensors_by_id.get(tensor_id)?;
    let config = tensor_configs_by_id.get(tensor_id)?;
    let views = node_views.get(compute_node)?;
    edge_index
        .and_then(|index| views.get(index)?.as_ref())
        .map_or(Some(tensor.num_bytes), |view| {
            view_physical_bytes(config, view)
        })
}

enum TensorViewDirection {
    Input,
    Output,
}

fn tensor_node_bytes(
    tensor_id: &str,
    node_id: &str,
    edge_index: Option<usize>,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    index: &TimetableIndex,
    direction: &TensorViewDirection,
) -> Option<u64> {
    let node_views = match direction {
        TensorViewDirection::Input => &index.node_input_views,
        TensorViewDirection::Output => &index.node_output_views,
    };
    tensor_view_bytes(
        tensor_id,
        node_id,
        edge_index,
        tensors_by_id,
        &index.tensor_configs,
        node_views,
    )
    .or_else(|| {
        memory_view_bytes(
            tensor_id,
            tensors_by_id,
            &index.tensor_configs,
            index.node_memory_configs.get(node_id),
        )
    })
}

fn memory_view_bytes(
    tensor_id: &str,
    tensors_by_id: &BTreeMap<String, TensorSummary>,
    tensor_configs_by_id: &BTreeMap<String, TensorConfigSection>,
    memory_config: Option<&MemoryConfigSection>,
) -> Option<u64> {
    let tensor = tensors_by_id.get(tensor_id)?;
    let config = tensor_configs_by_id.get(tensor_id)?;
    memory_config?
        .view
        .as_ref()
        .map_or(Some(tensor.num_bytes), |view| {
            view_physical_bytes(config, view)
        })
}

fn view_physical_bytes(config: &TensorConfigSection, view: &TensorViewSection) -> Option<u64> {
    let offset = view_element_offset(&config.shape, &view.offsets)?;
    let elements = u64::try_from(view.num_elements()).ok()?;
    let range = checked_dtype_byte_range(&config.dtype, offset, elements)?;
    Some(range.end - range.start)
}

fn view_element_offset(shape: &[usize], offsets: &[usize]) -> Option<u64> {
    if offsets.len() != shape.len() {
        return None;
    }
    offsets
        .iter()
        .enumerate()
        .try_fold(0_u64, |total, (dim, offset)| {
            let stride = shape[(dim + 1)..]
                .iter()
                .try_fold(1_u64, |elements, dimension| {
                    elements.checked_mul(u64::try_from(*dimension).ok()?)
                })?;
            total.checked_add(u64::try_from(*offset).ok()?.checked_mul(stride)?)
        })
}

fn push_traffic(
    consumption_by_pe: &mut Vec<TensorPeConsumption>,
    pe: Option<&str>,
    bytes: Option<u64>,
    layer: Option<&str>,
) {
    let pe = pe.unwrap_or("unassigned").to_string();
    let bytes = bytes.unwrap_or_default();
    if let Some(entry) = consumption_by_pe.iter_mut().find(|entry| entry.pe == pe) {
        entry.bytes = entry.bytes.saturating_add(bytes);
        entry.edge_count += 1;
        if let Some(layer) = layer {
            let traffic = entry.by_layer.entry(layer.to_string()).or_default();
            traffic.bytes = traffic.bytes.saturating_add(bytes);
            traffic.edge_count += 1;
        }
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
        pe,
        bytes,
        edge_count: 1,
        by_layer,
    });
    consumption_by_pe.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.pe.cmp(&b.pe)));
}
