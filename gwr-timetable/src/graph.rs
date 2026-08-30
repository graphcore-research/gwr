// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::operators::{Tensor, TensorView};
use gwr_models::processing_element::task::ComputeOp;

use crate::timetable_file::{
    EdgeKind, NodeSection, TensorViewSection, TimetableFile, parse_edge_end,
};

mod accesses;

/// A timetable whose nodes, edges, tensors, and views have been resolved and
/// validated.
#[derive(Debug)]
pub struct TimetableGraph {
    nodes: Vec<TimetableNode>,
    edges: Vec<TimetableEdge>,
    topological_order: Vec<usize>,
    topological_positions: Vec<usize>,
}

impl TimetableGraph {
    pub(crate) fn build(file: TimetableFile) -> Result<Self, SimError> {
        use ComputeTensorDirection::{Input, Output};

        let node_indices = node_indices(&file.nodes)?;
        let mut nodes = Vec::with_capacity(file.nodes.len());
        let mut view_sections = Vec::with_capacity(file.nodes.len());
        for section in file.nodes {
            let (node, views) = TimetableNode::new(section)?;
            nodes.push(node);
            view_sections.push(views);
        }
        let mut used_inputs = vec![EdgeIndexUsage::default(); nodes.len()];
        let mut used_outputs = vec![EdgeIndexUsage::default(); nodes.len()];
        let mut edges = Vec::with_capacity(file.edges.len());

        for section in file.edges {
            let edge_index = edges.len();
            let (from_id, requested_from_index) = parse_edge_end(&section.from)?;
            let (to_id, requested_to_index) = parse_edge_end(&section.to)?;
            let from_node = node_indices.get(from_id).copied().ok_or_else(|| {
                SimError(format!(
                    "Edge contains invalid from Node ID '{}'",
                    section.from
                ))
            })?;
            let to_node = node_indices.get(to_id).copied().ok_or_else(|| {
                SimError(format!("Edge contains invalid to Node ID '{}'", section.to))
            })?;

            if section.kind == EdgeKind::Control {
                edges.push(TimetableEdge {
                    kind: section.kind,
                    from: EdgeEndpoint::new(from_node, requested_from_index),
                    to: EdgeEndpoint::new(to_node, requested_to_index),
                    tensor: None,
                });
                continue;
            }

            let from_is_tensor = nodes[from_node].tensor().is_some();
            let to_is_tensor = nodes[to_node].tensor().is_some();
            if from_is_tensor == to_is_tensor {
                return invalid_data_edge(&nodes[from_node], &nodes[to_node], from_is_tensor);
            }

            let from_index = resolve_edge_index(
                &nodes[from_node],
                EdgeDirection::Output,
                requested_from_index,
                &mut used_outputs[from_node],
            )?;
            let to_index = resolve_edge_index(
                &nodes[to_node],
                EdgeDirection::Input,
                requested_to_index,
                &mut used_inputs[to_node],
            )?;

            nodes[from_node].successors.push(to_node);
            nodes[to_node].predecessors.push(from_node);

            let (tensor_node, compute_node, compute_tensor_index, direction) = if from_is_tensor {
                nodes[to_node].input_edges[to_index] = Some(edge_index);
                (from_node, to_node, to_index, Input)
            } else {
                nodes[from_node].output_edges[from_index] = Some(edge_index);
                (to_node, from_node, from_index, Output)
            };
            let view = make_view(
                &nodes,
                &view_sections,
                tensor_node,
                compute_node,
                compute_tensor_index,
                direction,
            )?;

            edges.push(TimetableEdge {
                kind: section.kind,
                from: EdgeEndpoint::new(from_node, Some(from_index)),
                to: EdgeEndpoint::new(to_node, Some(to_index)),
                tensor: Some(TensorConnection {
                    tensor_node,
                    compute_node,
                    compute_tensor_index,
                    direction,
                    view,
                }),
            });
        }

        validate_disconnected_views(&nodes, &view_sections)?;
        let topological_order = data_topological_order(&nodes)?;
        let mut topological_positions = vec![0; nodes.len()];
        for (position, node) in topological_order.iter().enumerate() {
            topological_positions[*node] = position;
        }
        let graph = Self {
            nodes,
            edges,
            topological_order,
            topological_positions,
        };
        graph.validate_operators()?;
        accesses::validate(&graph)?;
        Ok(graph)
    }

    #[must_use]
    pub fn nodes(&self) -> &[TimetableNode] {
        &self.nodes
    }

    #[must_use]
    pub fn edges(&self) -> &[TimetableEdge] {
        &self.edges
    }

    #[must_use]
    pub fn topological_order(&self) -> &[usize] {
        &self.topological_order
    }

    pub(super) fn topological_position(&self, node: usize) -> usize {
        self.topological_positions[node]
    }

    #[must_use]
    pub fn compute_views(&self, node_index: usize) -> Option<ComputeTensorViews> {
        let node = self.nodes.get(node_index)?;
        node.operation()?;
        Some(ComputeTensorViews {
            inputs: node
                .input_edges
                .iter()
                .map(|edge| {
                    edge.and_then(|edge| self.edges[edge].tensor.as_ref())
                        .map(|connection| connection.view.clone())
                })
                .collect(),
            outputs: node
                .output_edges
                .iter()
                .map(|edge| {
                    edge.and_then(|edge| self.edges[edge].tensor.as_ref())
                        .map(|connection| connection.view.clone())
                })
                .collect(),
        })
    }

    fn validate_operators(&self) -> SimResult {
        for (node_index, node) in self.nodes.iter().enumerate() {
            let Some(operation) = node.operation() else {
                continue;
            };
            let Some(views) = self.compute_views(node_index) else {
                return sim_error!("Node '{}' has no compute views", node.id());
            };
            operation
                .validate(views.inputs(), views.outputs())
                .map_err(|error| SimError(format!("Compute node '{}': {error}", node.id())))?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct TimetableNode {
    id: String,
    kind: TimetableNodeKind,
    input_edges: Vec<Option<usize>>,
    output_edges: Vec<Option<usize>>,
    predecessors: Vec<usize>,
    successors: Vec<usize>,
}

#[derive(Debug)]
enum TimetableNodeKind {
    Compute {
        operation: ComputeOp,
        pe: Option<String>,
    },
    Tensor(Tensor),
}

struct ComputeViewSections {
    inputs: Vec<Option<TensorViewSection>>,
    outputs: Vec<Option<TensorViewSection>>,
}

impl TimetableNode {
    fn new(section: NodeSection) -> Result<(Self, Option<ComputeViewSections>), SimError> {
        let (id, kind, views, num_inputs, num_outputs) = match section {
            NodeSection::Compute {
                id,
                op,
                pe,
                input_views,
                output_views,
            } => {
                let num_inputs = input_views.len();
                let num_outputs = output_views.len();
                (
                    id,
                    TimetableNodeKind::Compute { operation: op, pe },
                    Some(ComputeViewSections {
                        inputs: input_views,
                        outputs: output_views,
                    }),
                    num_inputs,
                    num_outputs,
                )
            }
            NodeSection::Tensor { id, config } => {
                let tensor = Tensor::new(&config.shape, &config.dtype, config.addr)
                    .map_err(|error| SimError(format!("Tensor '{id}': {error}")))?
                    .with_id(&id);
                (id, TimetableNodeKind::Tensor(tensor), None, 0, 0)
            }
        };
        Ok((
            Self {
                id,
                kind,
                input_edges: vec![None; num_inputs],
                output_edges: vec![None; num_outputs],
                predecessors: Vec::new(),
                successors: Vec::new(),
            },
            views,
        ))
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn pe(&self) -> Option<&str> {
        match &self.kind {
            TimetableNodeKind::Compute { pe, .. } => pe.as_deref(),
            TimetableNodeKind::Tensor(_) => None,
        }
    }

    #[must_use]
    pub fn operation(&self) -> Option<&ComputeOp> {
        match &self.kind {
            TimetableNodeKind::Compute { operation, .. } => Some(operation),
            TimetableNodeKind::Tensor(_) => None,
        }
    }

    #[must_use]
    pub fn tensor(&self) -> Option<&Tensor> {
        match &self.kind {
            TimetableNodeKind::Compute { .. } => None,
            TimetableNodeKind::Tensor(tensor) => Some(tensor),
        }
    }

    #[must_use]
    pub fn input_edges(&self) -> &[Option<usize>] {
        &self.input_edges
    }

    #[must_use]
    pub fn output_edges(&self) -> &[Option<usize>] {
        &self.output_edges
    }

    #[must_use]
    pub fn predecessors(&self) -> &[usize] {
        &self.predecessors
    }

    #[must_use]
    pub fn successors(&self) -> &[usize] {
        &self.successors
    }
}

#[derive(Debug)]
pub struct TimetableEdge {
    kind: EdgeKind,
    from: EdgeEndpoint,
    to: EdgeEndpoint,
    tensor: Option<TensorConnection>,
}

impl TimetableEdge {
    #[must_use]
    pub fn kind(&self) -> EdgeKind {
        self.kind
    }

    #[must_use]
    pub fn from(&self) -> EdgeEndpoint {
        self.from
    }

    #[must_use]
    pub fn to(&self) -> EdgeEndpoint {
        self.to
    }

    #[must_use]
    pub fn tensor_connection(&self) -> Option<&TensorConnection> {
        self.tensor.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdgeEndpoint {
    node: usize,
    edge_index: Option<usize>,
}

impl EdgeEndpoint {
    fn new(node: usize, edge_index: Option<usize>) -> Self {
        Self { node, edge_index }
    }

    #[must_use]
    pub fn node(&self) -> usize {
        self.node
    }

    #[must_use]
    pub fn edge_index(&self) -> Option<usize> {
        self.edge_index
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeTensorDirection {
    Input,
    Output,
}

#[derive(Debug)]
pub struct TensorConnection {
    tensor_node: usize,
    compute_node: usize,
    compute_tensor_index: usize,
    direction: ComputeTensorDirection,
    view: TensorView,
}

impl TensorConnection {
    #[must_use]
    pub fn tensor_node(&self) -> usize {
        self.tensor_node
    }

    #[must_use]
    pub fn compute_node(&self) -> usize {
        self.compute_node
    }

    #[must_use]
    pub fn compute_tensor_index(&self) -> usize {
        self.compute_tensor_index
    }

    #[must_use]
    pub fn direction(&self) -> ComputeTensorDirection {
        self.direction
    }

    #[must_use]
    pub fn view(&self) -> &TensorView {
        &self.view
    }
}

#[derive(Debug)]
pub struct ComputeTensorViews {
    inputs: Vec<Option<TensorView>>,
    outputs: Vec<Option<TensorView>>,
}

impl ComputeTensorViews {
    #[must_use]
    pub fn inputs(&self) -> &[Option<TensorView>] {
        &self.inputs
    }

    #[must_use]
    pub fn outputs(&self) -> &[Option<TensorView>] {
        &self.outputs
    }

    #[must_use]
    pub fn into_parts(self) -> (Vec<Option<TensorView>>, Vec<Option<TensorView>>) {
        (self.inputs, self.outputs)
    }
}

fn node_indices(nodes: &[NodeSection]) -> Result<HashMap<String, usize>, SimError> {
    let mut indices = HashMap::new();
    let mut duplicates = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        if indices.insert(node.id().to_string(), index).is_some() {
            duplicates.push(format!("Duplicate Node ID '{}'", node.id()));
        }
    }
    if !duplicates.is_empty() {
        return sim_error!("Failed to validate graph:\n{}", duplicates.join("\n"));
    }
    Ok(indices)
}

#[derive(Clone, Copy)]
enum EdgeDirection {
    Input,
    Output,
}

#[derive(Clone, Default)]
struct EdgeIndexUsage {
    occupied_indices: HashSet<usize>,
    next_free_index: usize,
}

impl EdgeIndexUsage {
    fn claim(
        &mut self,
        node: &TimetableNode,
        direction: EdgeDirection,
        requested_index: Option<usize>,
    ) -> Result<usize, SimError> {
        let limit = match direction {
            EdgeDirection::Input => node.operation().map(|_| node.input_edges.len()),
            EdgeDirection::Output => node.operation().map(|_| node.output_edges.len()),
        };
        let tensor_index = requested_index.unwrap_or(self.next_free_index);
        if let Some(limit) = limit
            && tensor_index >= limit
        {
            return sim_error!(
                "Node '{}' {direction} tensor index {tensor_index} is out of range for {limit} declared {direction} tensors",
                node.id()
            );
        }
        if !self.occupied_indices.insert(tensor_index) {
            return sim_error!(
                "Node '{}' {direction} tensor index {tensor_index} is connected more than once",
                node.id()
            );
        }

        if tensor_index == self.next_free_index {
            loop {
                self.next_free_index = self.next_free_index.checked_add(1).ok_or_else(|| {
                    SimError(format!(
                        "Node '{}' {direction} tensor index overflows",
                        node.id()
                    ))
                })?;
                if !self.occupied_indices.contains(&self.next_free_index) {
                    break;
                }
            }
        }
        Ok(tensor_index)
    }
}

impl fmt::Display for EdgeDirection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input => formatter.write_str("input"),
            Self::Output => formatter.write_str("output"),
        }
    }
}

fn resolve_edge_index(
    node: &TimetableNode,
    direction: EdgeDirection,
    requested_index: Option<usize>,
    used: &mut EdgeIndexUsage,
) -> Result<usize, SimError> {
    used.claim(node, direction, requested_index)
}

fn validate_disconnected_views(
    nodes: &[TimetableNode],
    view_sections: &[Option<ComputeViewSections>],
) -> SimResult {
    for (node, sections) in nodes.iter().zip(view_sections) {
        let Some(sections) = sections else {
            continue;
        };
        for (direction, views, edges) in [
            ("input", &sections.inputs, &node.input_edges),
            ("output", &sections.outputs, &node.output_edges),
        ] {
            for (tensor_index, (view, edge)) in views.iter().zip(edges).enumerate() {
                if view.is_some() && edge.is_none() {
                    return sim_error!(
                        "Compute node '{}' declares an {direction} view for disconnected tensor index {tensor_index}",
                        node.id()
                    );
                }
            }
        }
    }
    Ok(())
}

fn data_topological_order(nodes: &[TimetableNode]) -> Result<Vec<usize>, SimError> {
    let mut unresolved_predecessors = nodes
        .iter()
        .map(|node| node.predecessors.len())
        .collect::<Vec<_>>();
    let mut ready = unresolved_predecessors
        .iter()
        .enumerate()
        .filter_map(|(node, count)| (*count == 0).then_some(node))
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(nodes.len());

    while let Some(node) = ready.pop_front() {
        order.push(node);
        for successor in &nodes[node].successors {
            unresolved_predecessors[*successor] -= 1;
            if unresolved_predecessors[*successor] == 0 {
                ready.push_back(*successor);
            }
        }
    }

    if order.len() == nodes.len() {
        return Ok(order);
    }
    let unresolved = unresolved_predecessors
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(node, _)| format!("'{}'", nodes[node].id()))
        .collect::<Vec<_>>();
    sim_error!(
        "Data dependency graph contains a cycle; unresolved nodes: {}",
        unresolved.join(", ")
    )
}

fn invalid_data_edge<T>(
    from: &TimetableNode,
    to: &TimetableNode,
    nodes_are_tensors: bool,
) -> Result<T, SimError> {
    if nodes_are_tensors {
        sim_error!(
            "Invalid edge from Tensor node '{}' to Tensor node '{}': tensors must be connected through compute nodes",
            from.id(),
            to.id()
        )
    } else {
        sim_error!(
            "Invalid data edge from compute node '{}' to compute node '{}': compute nodes must be connected through tensors",
            from.id(),
            to.id()
        )
    }
}

fn make_view(
    nodes: &[TimetableNode],
    view_sections: &[Option<ComputeViewSections>],
    tensor_node: usize,
    compute_node: usize,
    compute_tensor_index: usize,
    direction: ComputeTensorDirection,
) -> Result<TensorView, SimError> {
    let tensor = nodes[tensor_node].tensor().cloned().ok_or_else(|| {
        SimError(format!(
            "Data connection identifies compute node '{}' as a tensor",
            nodes[tensor_node].id()
        ))
    })?;
    let sections = view_sections[compute_node].as_ref().ok_or_else(|| {
        SimError(format!(
            "Data connection identifies tensor node '{}' as a compute node",
            nodes[compute_node].id()
        ))
    })?;
    let (direction_name, section) = match direction {
        ComputeTensorDirection::Input => ("input", &sections.inputs[compute_tensor_index]),
        ComputeTensorDirection::Output => ("output", &sections.outputs[compute_tensor_index]),
    };
    view_from_section(tensor, section.as_ref()).map_err(|error| {
        SimError(format!(
            "{direction_name} view on node '{}': {error}",
            nodes[compute_node].id()
        ))
    })
}

fn view_from_section(
    tensor: Tensor,
    section: Option<&TensorViewSection>,
) -> Result<TensorView, SimError> {
    match section {
        Some(section) => TensorView::new(tensor, &section.shape, &section.offsets),
        None => Ok(TensorView::new_full(tensor)),
    }
}
