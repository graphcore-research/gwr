// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};

use gwr_timetable::timetable_file::{EdgeKind, EdgeSection, NodeSection, TimetableFile};

pub(super) fn compute_graph_layers(timetable: &TimetableFile) -> BTreeMap<String, usize> {
    let compute_ids = timetable
        .nodes
        .iter()
        .filter_map(|node| match node {
            NodeSection::Compute { id, .. } => Some(id.clone()),
            NodeSection::Tensor { .. } => None,
        })
        .collect::<BTreeSet<_>>();
    let root_tensor_ids = root_tensor_ids(timetable);
    let root_compute_ids = timetable
        .edges
        .iter()
        .filter(|edge| is_data_edge(edge))
        .filter_map(|edge| {
            let from = edge.from_node_id();
            let to = edge.to_node_id();
            (compute_ids.contains(to) && root_tensor_ids.contains(from)).then(|| to.to_string())
        })
        .collect::<BTreeSet<_>>();
    let layer_starts = if root_compute_ids.is_empty() {
        compute_ids.clone()
    } else {
        root_compute_ids
    };
    let mut layers = initial_layers(timetable, &layer_starts);
    propagate_layers(timetable, &layer_starts, &mut layers);
    continue_late_roots(timetable, &layer_starts, &mut layers);
    propagate_layers(timetable, &layer_starts, &mut layers);

    compute_ids
        .into_iter()
        .map(|id| (id.clone(), layers.get(&id).copied().unwrap_or_default()))
        .collect()
}

fn root_tensor_ids(timetable: &TimetableFile) -> BTreeSet<&str> {
    let tensor_ids = timetable
        .nodes
        .iter()
        .filter_map(|node| match node {
            NodeSection::Tensor { id, .. } => Some(id.as_str()),
            NodeSection::Compute { .. } => None,
        })
        .collect::<BTreeSet<_>>();
    let produced_tensor_ids = timetable
        .edges
        .iter()
        .filter(|edge| is_data_edge(edge))
        .map(EdgeSection::to_node_id)
        .filter(|id| tensor_ids.contains(id))
        .collect::<BTreeSet<_>>();

    tensor_ids
        .difference(&produced_tensor_ids)
        .copied()
        .collect()
}

fn continue_late_roots(
    timetable: &TimetableFile,
    layer_starts: &BTreeSet<String>,
    layers: &mut BTreeMap<String, usize>,
) {
    // A root encountered after the graph has advanced is assumed to continue
    // the model sequence. Earlier disconnected roots remain parallel.
    let mut highest_layer = 0;
    for node in &timetable.nodes {
        let NodeSection::Compute { id, .. } = node else {
            continue;
        };
        if !layer_starts.contains(id) {
            continue;
        }

        let layer = layers.get(id).copied().unwrap_or_default();
        let layer = if layer == 1 && highest_layer > 1 {
            highest_layer + 1
        } else {
            layer
        };
        layers.insert(id.clone(), layer);
        highest_layer = highest_layer.max(layer);
    }
}

fn initial_layers(
    timetable: &TimetableFile,
    layer_starts: &BTreeSet<String>,
) -> BTreeMap<String, usize> {
    timetable
        .nodes
        .iter()
        .map(|node| {
            let id = match node {
                NodeSection::Compute { id, .. } | NodeSection::Tensor { id, .. } => id,
            };
            (id.clone(), usize::from(layer_starts.contains(id)))
        })
        .collect()
}

fn propagate_layers(
    timetable: &TimetableFile,
    layer_starts: &BTreeSet<String>,
    layers: &mut BTreeMap<String, usize>,
) {
    let mut incoming_edges = layers
        .keys()
        .map(|id| (id.clone(), 0_usize))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing_edges = BTreeMap::<String, Vec<String>>::new();
    for edge in timetable.edges.iter().filter(|edge| is_data_edge(edge)) {
        let from = edge.from_node_id();
        let to = edge.to_node_id();
        if !layers.contains_key(from) || !layers.contains_key(to) {
            continue;
        }
        *incoming_edges.get_mut(to).expect("known node") += 1;
        outgoing_edges
            .entry(from.to_string())
            .or_default()
            .push(to.to_string());
    }

    let mut ready = incoming_edges
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
        .collect::<BTreeSet<_>>();
    while let Some(id) = ready.pop_first() {
        let layer = layers.get(&id).copied().unwrap_or_default();
        for target in outgoing_edges.get(&id).into_iter().flatten() {
            let candidate = layer + usize::from(layer_starts.contains(target));
            let target_layer = layers.get_mut(target).expect("known node");
            *target_layer = (*target_layer).max(candidate);

            let count = incoming_edges.get_mut(target).expect("known node");
            *count -= 1;
            if *count == 0 {
                ready.insert(target.clone());
            }
        }
    }
}

pub(super) fn layer_name(layer: usize) -> String {
    if layer == 0 {
        "pre-layer".to_string()
    } else {
        format!("layer {layer}")
    }
}

pub(super) fn is_data_edge(edge: &EdgeSection) -> bool {
    matches!(&edge.kind, EdgeKind::Data)
}
