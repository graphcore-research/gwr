// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::memory::checked_last_address;
use gwr_models::processing_element::operators::{Tensor, TensorView};

use crate::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
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
            "Invalid edge from Tensor node '{}' to Tensor node '{}': tensors must be connected through compute nodes",
            from_node.node_section.id(),
            to_node.node_section.id()
        );
    }

    Ok(())
}

pub(crate) fn validate_nodes(nodes: &[Node]) -> SimResult {
    validate_acyclic(nodes)?;
    let mut access_groups = (0..nodes.len()).map(|_| None).collect::<Vec<_>>();
    for (node_idx, node) in nodes.iter().enumerate() {
        match &node.node_section {
            NodeSection::Compute {
                id,
                input_views,
                output_views,
                ..
            } => validate_compute_node(nodes, node, id, input_views, output_views)?,
            NodeSection::Tensor { id, config } => {
                access_groups[node_idx] = Some(TensorAccessGroup::new(node_idx, id, config)?);
            }
        }
    }
    for (node_idx, node) in nodes.iter().enumerate() {
        index_compute_accesses(node_idx, node, &mut access_groups)?;
    }
    validate_no_overlapping_accesses(nodes, &access_groups)
}

fn validate_acyclic(nodes: &[Node]) -> SimResult {
    let mut unresolved_predecessors = nodes
        .iter()
        .map(|node| node.predecessors.len())
        .collect::<Vec<_>>();
    let mut ready = unresolved_predecessors
        .iter()
        .enumerate()
        .filter_map(|(node_idx, count)| (*count == 0).then_some(node_idx))
        .collect::<VecDeque<_>>();

    while let Some(node_idx) = ready.pop_front() {
        for successor_idx in &nodes[node_idx].successors {
            unresolved_predecessors[*successor_idx] -= 1;
            if unresolved_predecessors[*successor_idx] == 0 {
                ready.push_back(*successor_idx);
            }
        }
    }

    let unresolved = unresolved_predecessors
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(node_idx, _)| format!("'{}'", nodes[node_idx].node_section.id()))
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        return sim_error!(
            "Data dependency graph contains a cycle; unresolved nodes: {}",
            unresolved.join(", ")
        );
    }
    Ok(())
}

struct TensorAccessGroup<'a> {
    tensor_idx: usize,
    tensor_id: &'a str,
    tensor: Tensor,
    reads: Vec<NodeAccess<'a>>,
    writes: Vec<NodeAccess<'a>>,
}

impl<'a> TensorAccessGroup<'a> {
    fn new(
        tensor_idx: usize,
        tensor_id: &'a str,
        config: &TensorConfigSection,
    ) -> Result<Self, SimError> {
        if let Some(dim) = config.shape.iter().position(|size| *size == 0) {
            return Err(SimError(format!(
                "Tensor '{tensor_id}' has zero size in dim {dim}"
            )));
        }
        let tensor = Tensor::new(&config.shape, &config.dtype, config.addr)
            .map_err(|error| SimError(format!("Tensor '{tensor_id}': {error}")))?;
        absolute_byte_range(tensor_id, config.addr, 0..tensor.num_bytes())?;
        Ok(Self {
            tensor_idx,
            tensor_id,
            tensor,
            reads: Vec::new(),
            writes: Vec::new(),
        })
    }
}

struct NodeAccess<'a> {
    node_idx: usize,
    node_id: &'a str,
    access: TensorAccess<'a>,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum AccessDirection {
    Read,
    Write,
}

fn index_compute_accesses<'a>(
    node_idx: usize,
    node: &'a Node,
    access_groups: &mut [Option<TensorAccessGroup<'a>>],
) -> SimResult {
    let NodeSection::Compute {
        id,
        input_views,
        output_views,
        ..
    } = &node.node_section
    else {
        return Ok(());
    };

    index_accesses(
        node_idx,
        id,
        &node.inputs,
        input_views,
        access_groups,
        AccessDirection::Read,
    )?;
    index_accesses(
        node_idx,
        id,
        &node.outputs,
        output_views,
        access_groups,
        AccessDirection::Write,
    )
}

fn index_accesses<'a>(
    node_idx: usize,
    node_id: &'a str,
    tensor_indices: &[Option<usize>],
    views: &[Option<TensorViewSection>],
    access_groups: &mut [Option<TensorAccessGroup<'a>>],
    direction: AccessDirection,
) -> SimResult {
    for (tensor_idx, view) in tensor_indices.iter().zip(views) {
        let Some(tensor_idx) = tensor_idx else {
            continue;
        };
        let Some(group) = access_groups[*tensor_idx].as_mut() else {
            continue;
        };
        let access = tensor_access(group.tensor_id, &group.tensor, view.as_ref())?;
        let access = NodeAccess {
            node_idx,
            node_id,
            access,
        };
        match direction {
            AccessDirection::Read => group.reads.push(access),
            AccessDirection::Write => group.writes.push(access),
        }
    }
    Ok(())
}

fn validate_no_overlapping_accesses(
    nodes: &[Node],
    access_groups: &[Option<TensorAccessGroup<'_>>],
) -> SimResult {
    let mut accesses = Vec::new();
    for group in access_groups.iter().filter_map(Option::as_ref) {
        accesses.extend(
            group
                .reads
                .iter()
                .map(|access| (group, access, AccessDirection::Read)),
        );
        accesses.extend(
            group
                .writes
                .iter()
                .map(|access| (group, access, AccessDirection::Write)),
        );
    }
    accesses.sort_by_key(|(_, access, _)| access.access.bounds.start);

    let mut dependencies = DependencyOrder::new(nodes);
    let mut active: Vec<(&TensorAccessGroup<'_>, &NodeAccess<'_>, AccessDirection)> = Vec::new();
    for current in accesses {
        active.retain(|(_, access, _)| access.access.bounds.end > current.1.access.bounds.start);
        for candidate in &active {
            if candidate.2 == AccessDirection::Read && current.2 == AccessDirection::Read {
                continue;
            }
            validate_access_pair(
                candidate.0,
                candidate.1,
                candidate.2,
                current.0,
                current.1,
                current.2,
                &mut dependencies,
            )?;
        }
        active.push(current);
    }
    Ok(())
}

fn validate_access_pair(
    first_group: &TensorAccessGroup<'_>,
    first: &NodeAccess<'_>,
    first_direction: AccessDirection,
    second_group: &TensorAccessGroup<'_>,
    second: &NodeAccess<'_>,
    second_direction: AccessDirection,
    dependencies: &mut DependencyOrder<'_>,
) -> SimResult {
    let Some((first_range, second_range)) =
        first_overlapping_ranges(&first.access, &second.access)?
    else {
        return Ok(());
    };

    if first.node_idx != second.node_idx {
        let excluded_tensor = (first_direction == AccessDirection::Write
            && second_direction == AccessDirection::Write
            && first_group.tensor_idx == second_group.tensor_idx)
            .then_some(first_group.tensor_idx);
        if dependencies.are_ordered(first.node_idx, second.node_idx, excluded_tensor) {
            return Ok(());
        }
    }

    overlapping_access_error(
        first_group,
        first,
        first_direction,
        first_range,
        second_group,
        second,
        second_direction,
        second_range,
    )
}

#[allow(clippy::too_many_arguments)]
fn overlapping_access_error(
    first_group: &TensorAccessGroup<'_>,
    first: &NodeAccess<'_>,
    first_direction: AccessDirection,
    first_range: Range<u128>,
    second_group: &TensorAccessGroup<'_>,
    second: &NodeAccess<'_>,
    second_direction: AccessDirection,
    second_range: Range<u128>,
) -> SimResult {
    match (first_direction, second_direction) {
        (AccessDirection::Read, AccessDirection::Write) if first.node_idx == second.node_idx => {
            sim_error!(
                "Node '{}' reads tensor '{}' from memory range {:#x}..{:#x} and writes tensor '{}' to overlapping range {:#x}..{:#x}",
                first.node_id,
                first_group.tensor_id,
                first_range.start,
                first_range.end,
                second_group.tensor_id,
                second_range.start,
                second_range.end,
            )
        }
        (AccessDirection::Write, AccessDirection::Read) if first.node_idx == second.node_idx => {
            sim_error!(
                "Node '{}' reads tensor '{}' from memory range {:#x}..{:#x} and writes tensor '{}' to overlapping range {:#x}..{:#x}",
                second.node_id,
                second_group.tensor_id,
                second_range.start,
                second_range.end,
                first_group.tensor_id,
                first_range.start,
                first_range.end,
            )
        }
        (AccessDirection::Read, AccessDirection::Write) => sim_error!(
            "Node '{}' reads tensor '{}' from memory range {:#x}..{:#x} while unordered node '{}' writes tensor '{}' to overlapping range {:#x}..{:#x}",
            first.node_id,
            first_group.tensor_id,
            first_range.start,
            first_range.end,
            second.node_id,
            second_group.tensor_id,
            second_range.start,
            second_range.end,
        ),
        (AccessDirection::Write, AccessDirection::Read) => sim_error!(
            "Node '{}' reads tensor '{}' from memory range {:#x}..{:#x} while unordered node '{}' writes tensor '{}' to overlapping range {:#x}..{:#x}",
            second.node_id,
            second_group.tensor_id,
            second_range.start,
            second_range.end,
            first.node_id,
            first_group.tensor_id,
            first_range.start,
            first_range.end,
        ),
        (AccessDirection::Write, AccessDirection::Write) if first.node_idx == second.node_idx => {
            sim_error!(
                "Node '{}' writes tensor '{}' to memory range {:#x}..{:#x} and tensor '{}' to overlapping range {:#x}..{:#x}",
                first.node_id,
                first_group.tensor_id,
                first_range.start,
                first_range.end,
                second_group.tensor_id,
                second_range.start,
                second_range.end,
            )
        }
        (AccessDirection::Write, AccessDirection::Write)
            if first_group.tensor_idx == second_group.tensor_idx =>
        {
            sim_error!(
                "Nodes '{}' and '{}' write tensor '{}' to overlapping memory ranges {:#x}..{:#x} and {:#x}..{:#x}",
                first.node_id,
                second.node_id,
                first_group.tensor_id,
                first_range.start,
                first_range.end,
                second_range.start,
                second_range.end,
            )
        }
        (AccessDirection::Write, AccessDirection::Write) => sim_error!(
            "Nodes '{}' and '{}' write tensors '{}' and '{}' to overlapping memory ranges {:#x}..{:#x} and {:#x}..{:#x}",
            first.node_id,
            second.node_id,
            first_group.tensor_id,
            second_group.tensor_id,
            first_range.start,
            first_range.end,
            second_range.start,
            second_range.end,
        ),
        (AccessDirection::Read, AccessDirection::Read) => Ok(()),
    }
}

struct DependencyOrder<'a> {
    nodes: &'a [Node],
    cache: HashMap<(usize, usize, Option<usize>), bool>,
}

impl<'a> DependencyOrder<'a> {
    fn new(nodes: &'a [Node]) -> Self {
        Self {
            nodes,
            cache: HashMap::new(),
        }
    }

    fn are_ordered(&mut self, first: usize, second: usize, excluded: Option<usize>) -> bool {
        let (first, second) = if first < second {
            (first, second)
        } else {
            (second, first)
        };
        *self
            .cache
            .entry((first, second, excluded))
            .or_insert_with(|| {
                has_dependency_path(self.nodes, first, second, excluded)
                    || has_dependency_path(self.nodes, second, first, excluded)
            })
    }
}

fn has_dependency_path(nodes: &[Node], from: usize, to: usize, excluded: Option<usize>) -> bool {
    let mut visited = vec![false; nodes.len()];
    let mut pending = vec![from];
    visited[from] = true;

    while let Some(node_idx) = pending.pop() {
        for successor in &nodes[node_idx].successors {
            if Some(*successor) == excluded || visited[*successor] {
                continue;
            }
            if *successor == to {
                return true;
            }
            visited[*successor] = true;
            pending.push(*successor);
        }
    }
    false
}

fn ranges_overlap(first: &Range<u128>, second: &Range<u128>) -> bool {
    first.start < second.end && second.start < first.end
}

struct TensorAccess<'a> {
    tensor_id: &'a str,
    view: TensorView,
    bounds: Range<u128>,
}

type OverlappingRanges = (Range<u128>, Range<u128>);

impl TensorAccess<'_> {
    fn ranges(&self) -> impl Iterator<Item = Result<Range<u128>, SimError>> + '_ {
        self.view
            .byte_ranges()
            .map(|range| absolute_byte_range(self.tensor_id, self.view.tensor().addr(), range))
    }
}

fn first_overlapping_ranges(
    first: &TensorAccess<'_>,
    second: &TensorAccess<'_>,
) -> Result<Option<OverlappingRanges>, SimError> {
    let mut first_ranges = first.ranges();
    let mut second_ranges = second.ranges();
    let Some(mut first_range) = first_ranges.next().transpose()? else {
        return Ok(None);
    };
    let Some(mut second_range) = second_ranges.next().transpose()? else {
        return Ok(None);
    };

    loop {
        if ranges_overlap(&first_range, &second_range) {
            return Ok(Some((first_range, second_range)));
        }
        if first_range.end <= second_range.start {
            let Some(next) = first_ranges.next().transpose()? else {
                return Ok(None);
            };
            first_range = next;
        } else {
            let Some(next) = second_ranges.next().transpose()? else {
                return Ok(None);
            };
            second_range = next;
        }
    }
}

fn tensor_access<'a>(
    id: &'a str,
    tensor: &Tensor,
    view: Option<&TensorViewSection>,
) -> Result<TensorAccess<'a>, SimError> {
    let view = match view {
        Some(view) => TensorView::new(tensor.clone(), &view.shape, &view.offsets),
        None => Ok(TensorView::new_full(tensor.clone())),
    }
    .map_err(|error| SimError(format!("Tensor '{id}' view: {error}")))?;
    let bounds = absolute_byte_range(id, view.tensor().addr(), view.byte_bounds())?;

    Ok(TensorAccess {
        tensor_id: id,
        view,
        bounds,
    })
}

fn absolute_byte_range(
    id: &str,
    base_address: u64,
    range: Range<usize>,
) -> Result<Range<u128>, SimError> {
    let byte_offset = u64::try_from(range.start)
        .map_err(|_| SimError(format!("Tensor '{id}' range is too large")))?;
    let num_bytes = u64::try_from(range.len())
        .map_err(|_| SimError(format!("Tensor '{id}' range is too large")))?;
    let start = base_address.checked_add(byte_offset).ok_or_else(|| {
        SimError(format!(
            "Tensor '{id}' range overflows the physical address space"
        ))
    })?;
    let last_address = checked_last_address(start, num_bytes).ok_or_else(|| {
        SimError(format!(
            "Tensor '{id}' range overflows the physical address space"
        ))
    })?;

    Ok(u128::from(start)..(u128::from(last_address) + 1))
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
