// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::task::MemoryOp;

use crate::timetable_file::{
    EdgeKind, EdgeSection, MemoryConfigSection, NodeSection, TensorConfigSection,
    TensorViewSection, TimetableFile, checked_dtype_byte_range,
};
use crate::types::Node;

pub(crate) fn validate_file(timetable_file: &TimetableFile) -> SimResult {
    timetable_file.validate_structure()?;
    let node_idx_by_id = timetable_file
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id().clone(), index))
        .collect::<HashMap<_, _>>();
    let mut nodes = timetable_file
        .nodes
        .iter()
        .cloned()
        .map(Node::new)
        .collect::<Vec<_>>();
    wire_nodes(&mut nodes, &node_idx_by_id, &timetable_file.edges)?;
    validate_nodes(&nodes)
}

pub(crate) fn wire_nodes(
    nodes: &mut [Node],
    node_idx_by_id: &HashMap<String, usize>,
    edges: &[EdgeSection],
) -> SimResult {
    for edge in edges {
        let (from_node_id, from_edge_idx) = edge.from_node_and_edge()?;
        let from_node_idx = node_idx_by_id
            .get(from_node_id)
            .copied()
            .ok_or_else(|| SimError(format!("Unknown node '{from_node_id}'")))?;
        let (to_node_id, to_edge_idx) = edge.to_node_and_edge()?;
        let to_node_idx = node_idx_by_id
            .get(to_node_id)
            .copied()
            .ok_or_else(|| SimError(format!("Unknown node '{to_node_id}'")))?;
        if !matches!(&edge.kind, EdgeKind::Data) {
            continue;
        }

        validate_edge_node_kinds(&nodes[from_node_idx], &nodes[to_node_idx])?;

        let from_is_tensor = matches!(
            nodes[from_node_idx].node_section,
            NodeSection::Tensor { .. }
        );
        let to_is_tensor = matches!(nodes[to_node_idx].node_section, NodeSection::Tensor { .. });

        nodes[to_node_idx].predecessors.push(from_node_idx);
        nodes[from_node_idx].successors.push(to_node_idx);

        if !to_is_tensor {
            update_edge_indices(from_node_idx, to_edge_idx, &mut nodes[to_node_idx].inputs)
                .map_err(|error| {
                    SimError(format!(
                        "Node {from_node_idx} '{}': {error}",
                        nodes[from_node_idx].node_section.id()
                    ))
                })?;
        }
        if !from_is_tensor {
            update_edge_indices(
                to_node_idx,
                from_edge_idx,
                &mut nodes[from_node_idx].outputs,
            )
            .map_err(|error| {
                SimError(format!(
                    "Node {to_node_idx} '{}': {error}",
                    nodes[to_node_idx].node_section.id()
                ))
            })?;
        }
    }
    Ok(())
}

fn validate_edge_node_kinds(from_node: &Node, to_node: &Node) -> SimResult {
    if matches!(from_node.node_section, NodeSection::Tensor { .. })
        && matches!(to_node.node_section, NodeSection::Tensor { .. })
    {
        return sim_error!(
            "Invalid edge from Tensor node '{}' to Tensor node '{}': tensors must be connected through compute or memory nodes",
            from_node.node_section.id(),
            to_node.node_section.id()
        );
    }

    Ok(())
}

pub(crate) fn validate_nodes(nodes: &[Node]) -> SimResult {
    for node in nodes {
        match &node.node_section {
            NodeSection::Memory { id, op, config, .. } => match op {
                MemoryOp::Load => validate_load_node(nodes, id, node, config)?,
                MemoryOp::Store => validate_store_node(nodes, id, node, config)?,
            },
            NodeSection::Compute {
                id,
                input_views,
                output_views,
                ..
            } => validate_compute_node(nodes, node, id, input_views, output_views)?,
            NodeSection::Tensor { id, config } => validate_tensor_range(id, config)?,
        }
        validate_no_read_write_overlap(nodes, node)?;
    }
    Ok(())
}

fn validate_no_read_write_overlap(nodes: &[Node], node: &Node) -> SimResult {
    let (input_views, output_views): (Vec<_>, Vec<_>) = match &node.node_section {
        NodeSection::Compute {
            input_views,
            output_views,
            ..
        } => (
            input_views.iter().map(Option::as_ref).collect(),
            output_views.iter().map(Option::as_ref).collect(),
        ),
        NodeSection::Memory { config, .. } => (
            vec![config.view.as_ref(); node.inputs.len()],
            vec![config.view.as_ref(); node.outputs.len()],
        ),
        NodeSection::Tensor { .. } => return Ok(()),
    };

    let reads = tensor_accesses(nodes, &node.inputs, &input_views)?;
    let writes = tensor_accesses(nodes, &node.outputs, &output_views)?;
    for read in &reads {
        for write in &writes {
            if ranges_overlap(&read.range, &write.range) {
                return sim_error!(
                    "Node '{}' reads tensor '{}' from memory range {:#x}..{:#x} and writes tensor '{}' to overlapping range {:#x}..{:#x}",
                    node.node_section.id(),
                    read.tensor_id,
                    read.range.start,
                    read.range.end,
                    write.tensor_id,
                    write.range.start,
                    write.range.end,
                );
            }
        }
    }
    for (write_idx, first) in writes.iter().enumerate() {
        for second in &writes[(write_idx + 1)..] {
            if ranges_overlap(&first.range, &second.range) {
                return sim_error!(
                    "Node '{}' writes tensor '{}' to memory range {:#x}..{:#x} and tensor '{}' to overlapping range {:#x}..{:#x}",
                    node.node_section.id(),
                    first.tensor_id,
                    first.range.start,
                    first.range.end,
                    second.tensor_id,
                    second.range.start,
                    second.range.end,
                );
            }
        }
    }
    Ok(())
}

fn ranges_overlap(first: &Range<u128>, second: &Range<u128>) -> bool {
    first.start < second.end && second.start < first.end
}

struct TensorAccess<'a> {
    tensor_id: &'a str,
    range: Range<u128>,
}

fn tensor_accesses<'a>(
    nodes: &'a [Node],
    tensor_indices: &[Option<usize>],
    views: &[Option<&TensorViewSection>],
) -> Result<Vec<TensorAccess<'a>>, SimError> {
    let mut accesses = Vec::new();
    for (tensor_idx, view) in tensor_indices.iter().zip(views) {
        let Some(tensor_idx) = tensor_idx else {
            continue;
        };
        let NodeSection::Tensor { id, config } = &nodes[*tensor_idx].node_section else {
            continue;
        };
        accesses.push(TensorAccess {
            tensor_id: id,
            range: tensor_access_range(id, config, *view)?,
        });
    }
    Ok(accesses)
}

fn validate_tensor_range(id: &str, config: &TensorConfigSection) -> SimResult {
    if let Some(dim) = config.shape.iter().position(|size| *size == 0) {
        return sim_error!("Tensor '{id}' has zero size in dim {dim}");
    }
    tensor_access_range(id, config, None).map(|_| ())
}

fn tensor_access_range(
    id: &str,
    config: &TensorConfigSection,
    view: Option<&TensorViewSection>,
) -> Result<Range<u128>, SimError> {
    let offset_elements = match view {
        Some(view) => checked_element_offset(&config.shape, &view.offsets),
        None => Some(0),
    }
    .ok_or_else(|| SimError(format!("Tensor '{id}' view offset is too large")))?;
    let shape = view.map_or(config.shape.as_slice(), |view| view.shape.as_slice());
    let num_elements = checked_num_elements(shape)
        .ok_or_else(|| SimError(format!("Tensor '{id}' shape is too large")))?;
    let byte_range = checked_dtype_byte_range(&config.dtype, offset_elements, num_elements)
        .ok_or_else(|| SimError(format!("Tensor '{id}' size is too large")))?;
    let start = u128::from(config.addr) + u128::from(byte_range.start);
    let end = u128::from(config.addr) + u128::from(byte_range.end);
    if end > u128::from(u64::MAX) {
        return sim_error!("Tensor '{id}' range overflows the physical address space");
    }
    Ok(start..end)
}

fn checked_element_offset(shape: &[usize], offsets: &[usize]) -> Option<u64> {
    if offsets.len() != shape.len() {
        return None;
    }
    offsets
        .iter()
        .enumerate()
        .try_fold(0_u64, |total, (dim, offset)| {
            let stride = checked_num_elements(&shape[(dim + 1)..])?;
            let offset = u64::try_from(*offset).ok()?;
            total.checked_add(offset.checked_mul(stride)?)
        })
}

fn checked_num_elements(shape: &[usize]) -> Option<u64> {
    shape.iter().try_fold(1_u64, |elements, dimension| {
        elements.checked_mul(u64::try_from(*dimension).ok()?)
    })
}

pub(crate) fn tensor_config_for_memory_node<'a>(
    nodes: &'a [Node],
    node: &Node,
) -> Option<&'a TensorConfigSection> {
    let node_idx = node.get_memory_tensor_node_idx()?;
    let NodeSection::Tensor { config, .. } = &nodes[node_idx].node_section else {
        return None;
    };
    Some(config)
}

fn update_edge_indices(
    node_idx: usize,
    edge_idx: Option<usize>,
    edge_indices: &mut Vec<Option<usize>>,
) -> SimResult {
    if let Some(idx) = edge_idx {
        if (idx + 1) > edge_indices.len() {
            edge_indices.resize_with(idx + 1, || None);
        }
        if edge_indices[idx].is_some() {
            return sim_error!("edge index {idx} already connected");
        }
        edge_indices[idx] = Some(node_idx);
    } else if let Some(edge_idx) = edge_indices.iter_mut().find(|edge_idx| edge_idx.is_none()) {
        *edge_idx = Some(node_idx);
    } else {
        edge_indices.push(Some(node_idx));
    }
    Ok(())
}

fn validate_compute_node(
    nodes: &[Node],
    node: &Node,
    id: &str,
    input_views: &[Option<TensorViewSection>],
    output_views: &[Option<TensorViewSection>],
) -> SimResult {
    if node.inputs.len() != input_views.len() {
        return sim_error!(
            "Compute node '{}' has {} input edges but {} input views",
            id,
            node.inputs.len(),
            input_views.len()
        );
    }
    if node.outputs.len() != output_views.len() {
        return sim_error!(
            "Compute node '{}' has {} output edges but {} output views",
            id,
            node.outputs.len(),
            output_views.len()
        );
    }

    for (input_idx, tensor_idx) in node.inputs.iter().enumerate() {
        let Some(tensor_idx) = tensor_idx else {
            continue;
        };
        let NodeSection::Tensor { config, .. } = &nodes[*tensor_idx].node_section else {
            return sim_error!(
                "Compute node '{}' input {} is not connected from a Tensor node",
                id,
                input_idx
            );
        };
        validate_view_in_range(id, "input", input_views[input_idx].as_ref(), config)?;
    }
    for (output_idx, tensor_idx) in node.outputs.iter().enumerate() {
        let Some(tensor_idx) = tensor_idx else {
            continue;
        };
        let NodeSection::Tensor { config, .. } = &nodes[*tensor_idx].node_section else {
            return sim_error!(
                "Compute node '{}' output {} is not connected to a Tensor node",
                id,
                output_idx
            );
        };
        validate_view_in_range(id, "output", output_views[output_idx].as_ref(), config)?;
    }
    Ok(())
}

fn validate_load_node(
    nodes: &[Node],
    id: &str,
    node: &Node,
    config: &MemoryConfigSection,
) -> SimResult {
    if node.inputs.len() != 1 {
        return sim_error!("{} edges connect into Load node '{id}'", node.inputs.len());
    }
    if !node.outputs.is_empty() {
        return sim_error!(
            "{} data edges connect from Load node '{id}'",
            node.outputs.len()
        );
    }
    let Some(tensor_config) = tensor_config_for_memory_node(nodes, node) else {
        return sim_error!("Load node '{id}' not connected from Tensor node");
    };
    validate_access_in_range(id, "Load", config, tensor_config)
}

fn validate_store_node(
    nodes: &[Node],
    id: &str,
    node: &Node,
    config: &MemoryConfigSection,
) -> SimResult {
    if node.outputs.len() != 1 {
        return sim_error!(
            "{} edges connect from Store node '{id}'",
            node.outputs.len()
        );
    }
    let Some(tensor_config) = tensor_config_for_memory_node(nodes, node) else {
        return sim_error!("Store node '{id}' not connected to Tensor node");
    };
    validate_access_in_range(id, "Store", config, tensor_config)
}

fn validate_access_in_range(
    node_id: &str,
    direction: &str,
    mem_config: &MemoryConfigSection,
    tensor_config: &TensorConfigSection,
) -> SimResult {
    validate_view_in_range(node_id, direction, mem_config.view.as_ref(), tensor_config)
}

fn validate_view_in_range(
    node_id: &str,
    direction: &str,
    view: Option<&TensorViewSection>,
    tensor_config: &TensorConfigSection,
) -> SimResult {
    let Some(view) = view else {
        return Ok(());
    };
    if view.offsets.len() != tensor_config.shape.len() {
        return sim_error!(
            "{direction} view on node '{}' has offsets rank {} but tensor rank {}",
            node_id,
            view.offsets.len(),
            tensor_config.shape.len()
        );
    }
    if view.shape.len() != tensor_config.shape.len() {
        return sim_error!(
            "{direction} view on node '{}' has shape rank {} but tensor rank {}",
            node_id,
            view.shape.len(),
            tensor_config.shape.len()
        );
    }
    if let Some(dim) = view.shape.iter().position(|size| *size == 0) {
        return sim_error!(
            "{direction} view on node '{}' has zero size in dim {dim}",
            node_id,
        );
    }
    for (i, ((offset, size), tensor_dim)) in view
        .offsets
        .iter()
        .zip(view.shape.iter())
        .zip(tensor_config.shape.iter())
        .enumerate()
    {
        if offset
            .checked_add(*size)
            .is_none_or(|end| end > *tensor_dim)
        {
            return sim_error!(
                "{direction} view on node '{}' is out of range in dim {i}: offset {offset} + size {size} > {tensor_dim}",
                node_id,
            );
        }
    }
    Ok(())
}
