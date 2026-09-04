// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeSet;

use gwr_engine::types::SimError;
use gwr_timetable::TimetableGraph;

pub(super) fn compute_graph_layers(graph: &TimetableGraph) -> Result<Vec<usize>, SimError> {
    let compute_nodes = graph
        .nodes()
        .iter()
        .enumerate()
        .filter_map(|(index, node)| node.operation().map(|_| index))
        .collect::<BTreeSet<_>>();
    let root_tensors = graph
        .nodes()
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            (node.tensor().is_some() && node.predecessors().is_empty()).then_some(index)
        })
        .collect::<BTreeSet<_>>();
    let root_computes = compute_nodes
        .iter()
        .copied()
        .filter(|index| {
            graph.nodes()[*index]
                .predecessors()
                .iter()
                .any(|predecessor| root_tensors.contains(predecessor))
        })
        .collect::<BTreeSet<_>>();
    let layer_starts = if root_computes.is_empty() {
        compute_nodes.clone()
    } else {
        root_computes
    };
    let mut layers = graph
        .nodes()
        .iter()
        .enumerate()
        .map(|(index, _)| usize::from(layer_starts.contains(&index)))
        .collect::<Vec<_>>();

    propagate_layers(graph, &layer_starts, &mut layers)?;
    continue_late_roots(graph, &layer_starts, &mut layers)?;
    propagate_layers(graph, &layer_starts, &mut layers)?;
    Ok(layers)
}

pub(super) fn layer_name(layer: usize) -> String {
    if layer == 0 {
        "pre-layer".to_string()
    } else {
        format!("layer {layer}")
    }
}

fn continue_late_roots(
    graph: &TimetableGraph,
    layer_starts: &BTreeSet<usize>,
    layers: &mut [usize],
) -> Result<(), SimError> {
    // Treat a root that appears after the graph has advanced as the next
    // model stage. Earlier disconnected roots remain in the same layer.
    let mut highest_layer = 0usize;
    for (index, node) in graph.nodes().iter().enumerate() {
        if node.operation().is_none() || !layer_starts.contains(&index) {
            continue;
        }

        let layer = if layers[index] == 1 && highest_layer > 1 {
            highest_layer
                .checked_add(1)
                .ok_or_else(|| SimError("Timetable layer count overflows".to_string()))?
        } else {
            layers[index]
        };
        layers[index] = layer;
        highest_layer = highest_layer.max(layer);
    }
    Ok(())
}

fn propagate_layers(
    graph: &TimetableGraph,
    layer_starts: &BTreeSet<usize>,
    layers: &mut [usize],
) -> Result<(), SimError> {
    let mut incoming = graph
        .nodes()
        .iter()
        .map(|node| node.predecessors().len())
        .collect::<Vec<_>>();
    let mut ready = incoming
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect::<BTreeSet<_>>();

    while let Some(index) = ready.pop_first() {
        for successor in graph.nodes()[index].successors() {
            let increment = usize::from(layer_starts.contains(successor));
            let candidate = layers[index]
                .checked_add(increment)
                .ok_or_else(|| SimError("Timetable layer count overflows".to_string()))?;
            layers[*successor] = layers[*successor].max(candidate);

            incoming[*successor] -= 1;
            if incoming[*successor] == 0 {
                ready.insert(*successor);
            }
        }
    }
    Ok(())
}
