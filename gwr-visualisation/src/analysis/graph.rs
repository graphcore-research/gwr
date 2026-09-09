// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};

use gwr_timetable::timetable_file::{EdgeKind, EdgeSection, NodeSection, TimetableFile};

use super::model::{ComputeGroupSummary, ComputeNodeSummary, ControlEdgeSummary, GraphSummary};

pub(super) fn summarize_graph(
    timetable: &TimetableFile,
    compute_nodes: &[ComputeNodeSummary],
    compute_node_indices: &BTreeMap<String, usize>,
) -> GraphSummary {
    let mut partition_groups = BTreeMap::<(String, String, String), Vec<(usize, usize)>>::new();
    let mut groups = Vec::<(usize, ComputeGroupSummary)>::new();

    for (index, node) in compute_nodes.iter().enumerate() {
        if let Some((stem, partition)) = partition_stem(&node.id) {
            partition_groups
                .entry((stem.to_string(), node.op.clone(), node.layer.clone()))
                .or_default()
                .push((partition, index));
        } else {
            groups.push((
                index,
                ComputeGroupSummary {
                    id: node.id.clone(),
                    members: vec![index],
                },
            ));
        }
    }

    for ((stem, _, _), mut members) in partition_groups {
        members.sort_unstable();
        let first_index = members.iter().map(|(_, index)| *index).min().unwrap_or(0);
        let member_indices = members
            .into_iter()
            .map(|(_, index)| index)
            .collect::<Vec<_>>();
        let id = if member_indices.len() > 1 {
            stem
        } else {
            compute_nodes[member_indices[0]].id.clone()
        };
        groups.push((
            first_index,
            ComputeGroupSummary {
                id,
                members: member_indices,
            },
        ));
    }
    groups.sort_by_key(|(first_index, _)| *first_index);

    let control_edges = timetable
        .edges
        .iter()
        .filter(|edge| matches!(&edge.kind, EdgeKind::Control))
        .filter_map(|edge| {
            Some(ControlEdgeSummary {
                from: *compute_node_indices.get(edge.from_node_id())?,
                to: *compute_node_indices.get(edge.to_node_id())?,
            })
        })
        .collect();

    GraphSummary {
        compute_groups: groups.into_iter().map(|(_, group)| group).collect(),
        control_edges,
    }
}

fn partition_stem(id: &str) -> Option<(&str, usize)> {
    let (stem, partition) = id.rsplit_once("_part_")?;
    if stem.is_empty()
        || partition.is_empty()
        || !partition.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((stem, partition.parse().ok()?))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::model::ComputeNodeSummary;

    fn compute(id: &str, op: &str, layer: &str) -> ComputeNodeSummary {
        ComputeNodeSummary {
            id: id.to_string(),
            pe: "pe_0_0".to_string(),
            layer: layer.to_string(),
            op: op.to_string(),
            machine_ops: 0,
            dominant_machine_op: None,
        }
    }

    #[test]
    fn groups_numeric_partition_suffixes_conservatively() {
        let compute_nodes = vec![
            compute("conv_part_10", "gemm", "layer 1"),
            compute("conv_part_2", "gemm", "layer 1"),
            compute("single_part_0", "add", "layer 1"),
            compute("mixed_part_0", "add", "layer 1"),
            compute("mixed_part_1", "gemm", "layer 1"),
            compute("depth_part_0", "add", "layer 1"),
            compute("depth_part_1", "add", "layer 2"),
            compute("not_a_partition", "add", "layer 1"),
        ];
        let indices = compute_nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.clone(), index))
            .collect();
        let timetable = TimetableFile {
            nodes: Vec::new(),
            edges: vec![EdgeSection {
                from: "conv_part_2".to_string(),
                to: "not_a_partition".to_string(),
                kind: EdgeKind::Control,
            }],
        };

        let graph = summarize_graph(&timetable, &compute_nodes, &indices);
        let conv = graph
            .compute_groups
            .iter()
            .find(|group| group.id == "conv")
            .unwrap();
        assert_eq!(conv.members, [1, 0]);
        for singleton in [
            "single_part_0",
            "mixed_part_0",
            "mixed_part_1",
            "depth_part_0",
            "depth_part_1",
            "not_a_partition",
        ] {
            assert!(
                graph
                    .compute_groups
                    .iter()
                    .any(|group| group.id == singleton && group.members.len() == 1),
                "missing singleton group for {singleton}"
            );
        }
        assert_eq!(graph.control_edges, [ControlEdgeSummary { from: 1, to: 7 }]);
    }

    #[test]
    fn rejects_non_numeric_partition_suffixes() {
        assert_eq!(partition_stem("conv_part_12"), Some(("conv", 12)));
        assert_eq!(partition_stem("conv_part_last"), None);
        assert_eq!(partition_stem("conv_part_"), None);
        assert_eq!(partition_stem("_part_0"), None);
    }
}
