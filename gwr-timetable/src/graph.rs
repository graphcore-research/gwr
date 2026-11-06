// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::fmt::{Display, Formatter};
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::program::PreparedProgram;
use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::program::validate::RegionBindings;
use gwr_models::processing_element::tensors::{Tensor, TensorView};

use self::accesses::validate;
use crate::timetable_file::{
    EdgeKind, NodeSection, TensorViewSection, TimetableFile, parse_edge_end, validate_node_id,
    validate_port_name,
};

mod accesses;

/// A timetable whose nodes, edges, tensors, and views have been resolved and
/// validated.
#[derive(Debug)]
pub struct TimetableGraph {
    nodes: Vec<TimetableNode>,
    edges: Vec<TimetableEdge>,
    topological_order: Vec<usize>,
}

impl TimetableGraph {
    pub(crate) fn build(file: TimetableFile) -> Result<Self, SimError> {
        use ComputeTensorDirection::{Input, Output};

        let node_indices = node_indices(&file.nodes)?;
        let mut nodes = Vec::with_capacity(file.nodes.len());
        let mut view_sections = Vec::with_capacity(file.nodes.len());
        for section in file.nodes {
            let (node, views) = NodeDraft::new(section)?;
            nodes.push(node);
            view_sections.push(views);
        }
        let mut used_inputs = vec![0; nodes.len()];
        let mut used_outputs = vec![0; nodes.len()];
        let mut edges = Vec::with_capacity(file.edges.len());

        for section in file.edges {
            let edge_index = edges.len();
            let (from_id, requested_from_port) = parse_edge_end(&section.from)?;
            let (to_id, requested_to_port) = parse_edge_end(&section.to)?;
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
                edges.push(resolve_control_edge(
                    &nodes,
                    (from_node, requested_from_port),
                    (to_node, requested_to_port),
                )?);
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
                requested_from_port,
                &mut used_outputs[from_node],
            )?;
            let to_index = resolve_edge_index(
                &nodes[to_node],
                EdgeDirection::Input,
                requested_to_port,
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
        validate(&nodes, &edges, &topological_positions)?;
        let nodes = nodes
            .into_iter()
            .map(|node| node.bind(&edges))
            .collect::<Result<_, _>>()?;
        let graph = Self {
            nodes,
            edges,
            topological_order,
        };
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

    /// Borrow the dense, resolved input and output views of a compute node.
    #[must_use]
    pub fn compute_views(&self, node_index: usize) -> Option<&ComputeTensorViews> {
        match &self.nodes.get(node_index)?.kind {
            TimetableNodeKind::Compute { views, .. } => Some(views),
            TimetableNodeKind::Tensor(_) => None,
        }
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

    /// Declared source output port, or `None` for a plain node endpoint.
    /// Pass the graph that owns this edge.
    #[must_use]
    pub fn from_port_name<'a>(&self, graph: &'a TimetableGraph) -> Option<&'a str> {
        let program = graph.nodes.get(self.from.node)?.program()?;
        program
            .output_names()
            .get(self.from.edge_index?)
            .map(String::as_str)
    }

    /// Declared destination input port, or `None` for a plain node endpoint.
    /// Pass the graph that owns this edge.
    #[must_use]
    pub fn to_port_name<'a>(&self, graph: &'a TimetableGraph) -> Option<&'a str> {
        let program = graph.nodes.get(self.to.node)?.program()?;
        program
            .input_names()
            .get(self.to.edge_index?)
            .map(String::as_str)
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
    inputs: Vec<TensorView>,
    outputs: Vec<TensorView>,
}

impl ComputeTensorViews {
    #[must_use]
    pub fn inputs(&self) -> &[TensorView] {
        &self.inputs
    }

    #[must_use]
    pub fn outputs(&self) -> &[TensorView] {
        &self.outputs
    }
}

fn node_indices(nodes: &[NodeSection]) -> Result<HashMap<String, usize>, SimError> {
    let mut indices = HashMap::new();
    let mut duplicates = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        validate_node_id(node.id())?;
        if indices.insert(node.id().to_string(), index).is_some() {
            duplicates.push(format!("Duplicate Node ID '{}'", node.id()));
        }
    }
    if !duplicates.is_empty() {
        return sim_error!("Failed to validate graph:\n{}", duplicates.join("\n"));
    }
    Ok(indices)
}

fn resolve_control_edge(
    nodes: &[NodeDraft],
    from: (usize, Option<&str>),
    to: (usize, Option<&str>),
) -> Result<TimetableEdge, SimError> {
    let from_port = resolve_port(&nodes[from.0], EdgeDirection::Output, from.1, false)?;
    let to_port = resolve_port(&nodes[to.0], EdgeDirection::Input, to.1, false)?;
    Ok(TimetableEdge {
        kind: EdgeKind::Control,
        from: EdgeEndpoint::new(from.0, from_port),
        to: EdgeEndpoint::new(to.0, to_port),
        tensor: None,
    })
}

fn resolve_port(
    node: &NodeDraft,
    direction: EdgeDirection,
    requested: Option<&str>,
    required: bool,
) -> Result<Option<usize>, SimError> {
    let DraftKind::Compute { program, .. } = &node.kind else {
        if requested.is_some() {
            return sim_error!(
                "Tensor node '{}' endpoint must be a plain node ID",
                node.id()
            );
        }
        return Ok(None);
    };
    let Some(name) = requested else {
        if required {
            return sim_error!(
                "Compute node '{}' requires a named {direction} port",
                node.id()
            );
        }
        return Ok(None);
    };
    let names = direction.names(program);
    names
        .iter()
        .position(|port| port == name)
        .map(Some)
        .ok_or_else(|| {
            SimError(format!(
                "Node '{}' has unknown {direction} port '{name}'",
                node.id()
            ))
        })
}

fn resolve_edge_index(
    node: &NodeDraft,
    direction: EdgeDirection,
    requested: Option<&str>,
    next_tensor_index: &mut usize,
) -> Result<usize, SimError> {
    let Some(index) = resolve_port(node, direction, requested, true)? else {
        let index = *next_tensor_index;
        *next_tensor_index += 1;
        return Ok(index);
    };
    let edges = match direction {
        EdgeDirection::Input => &node.input_edges,
        EdgeDirection::Output => &node.output_edges,
    };
    if edges[index].is_some() {
        return sim_error!(
            "Node '{}' {direction} port '{}' is connected more than once",
            node.id(),
            requested.expect("resolved port")
        );
    }
    Ok(index)
}

#[derive(Clone, Copy)]
enum EdgeDirection {
    Input,
    Output,
}

impl EdgeDirection {
    fn names(self, program: &ProgramSpec) -> &[String] {
        match self {
            Self::Input => &program.inputs,
            Self::Output => &program.outputs,
        }
    }
}

impl Display for EdgeDirection {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input => formatter.write_str("input"),
            Self::Output => formatter.write_str("output"),
        }
    }
}

fn validate_disconnected_views(
    nodes: &[NodeDraft],
    view_sections: &[Option<ComputeViewSections>],
) -> SimResult {
    for (node, sections) in nodes.iter().zip(view_sections) {
        let Some(sections) = sections else {
            continue;
        };
        for (direction, views, edges) in [
            (EdgeDirection::Input, &sections.inputs, &node.input_edges),
            (EdgeDirection::Output, &sections.outputs, &node.output_edges),
        ] {
            for (tensor_index, (view, edge)) in views.iter().zip(edges).enumerate() {
                if view.is_some() && edge.is_none() {
                    return sim_error!(
                        "Compute node '{}' declares an {direction} view for disconnected port '{}'",
                        node.id(),
                        node.port_names(direction)[tensor_index]
                    );
                }
            }
        }
    }
    Ok(())
}

fn data_topological_order(nodes: &[NodeDraft]) -> Result<Vec<usize>, SimError> {
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
    from: &NodeDraft,
    to: &NodeDraft,
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
    nodes: &[NodeDraft],
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

#[derive(Debug)]
struct NodeDraft {
    id: String,
    kind: DraftKind,
    input_edges: Vec<Option<usize>>,
    output_edges: Vec<Option<usize>>,
    predecessors: Vec<usize>,
    successors: Vec<usize>,
}

impl NodeDraft {
    fn new(section: NodeSection) -> Result<(Self, Option<ComputeViewSections>), SimError> {
        let (id, kind, views, num_inputs, num_outputs) = match section {
            NodeSection::Compute {
                id,
                program,
                tiling,
                pe,
                input_views,
                output_views,
            } => {
                let program = *program;
                let mut names = HashSet::new();
                for name in program.inputs.iter().chain(&program.outputs) {
                    validate_port_name(name)?;
                    if !names.insert(name) {
                        return sim_error!(
                            "Compute node '{id}' has duplicate program port '{name}'"
                        );
                    }
                }
                let input_views =
                    resolve_views(&id, &program.inputs, input_views, EdgeDirection::Input)?;
                let output_views =
                    resolve_views(&id, &program.outputs, output_views, EdgeDirection::Output)?;
                let num_inputs = program.inputs.len();
                let num_outputs = program.outputs.len();
                (
                    id,
                    DraftKind::Compute {
                        program,
                        tiling,
                        pe,
                    },
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
                (id, DraftKind::Tensor(tensor), None, 0, 0)
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

    fn id(&self) -> &str {
        &self.id
    }
    fn port_names(&self, direction: EdgeDirection) -> &[String] {
        match &self.kind {
            DraftKind::Compute { program, .. } => direction.names(program),
            DraftKind::Tensor(_) => &[],
        }
    }
    fn tensor(&self) -> Option<&Tensor> {
        match &self.kind {
            DraftKind::Tensor(tensor) => Some(tensor),
            DraftKind::Compute { .. } => None,
        }
    }
    fn bind(self, edges: &[TimetableEdge]) -> Result<TimetableNode, SimError> {
        let dense_edges = |ports: &[Option<usize>], direction: EdgeDirection| {
            ports
                .iter()
                .enumerate()
                .map(|(port, edge)| {
                    edge.ok_or_else(|| {
                        SimError(format!(
                            "Compute node '{}' has an unbound program {direction} '{}'",
                            self.id,
                            self.port_names(direction)[port]
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let input_edges = dense_edges(&self.input_edges, EdgeDirection::Input)?;
        let output_edges = dense_edges(&self.output_edges, EdgeDirection::Output)?;
        let kind = match self.kind {
            DraftKind::Compute {
                program,
                tiling,
                pe,
            } => {
                let views_for = |ports: &[usize]| {
                    ports
                        .iter()
                        .map(|&edge| {
                            edges[edge]
                                .tensor
                                .as_ref()
                                .expect("resolved data edge")
                                .view
                                .clone()
                        })
                        .collect::<Vec<_>>()
                };
                let views = ComputeTensorViews {
                    inputs: views_for(&input_edges),
                    outputs: views_for(&output_edges),
                };
                let program = PreparedProgram::new(
                    &program,
                    RegionBindings {
                        inputs: views.inputs.clone(),
                        outputs: views.outputs.clone(),
                    },
                    tiling.as_ref(),
                )
                .map_err(|error| SimError(format!("Compute node '{}': {error}", self.id)))?;
                TimetableNodeKind::Compute {
                    program: Rc::new(program),
                    pe,
                    views,
                }
            }
            DraftKind::Tensor(tensor) => TimetableNodeKind::Tensor(tensor),
        };
        Ok(TimetableNode {
            id: self.id,
            kind,
            input_edges,
            output_edges,
            predecessors: self.predecessors,
            successors: self.successors,
        })
    }
}

#[derive(Debug)]
enum DraftKind {
    Compute {
        program: ProgramSpec,
        tiling: Option<TileSpec>,
        pe: Option<String>,
    },
    Tensor(Tensor),
}

/// A resolved tensor or compute node with dense data ports.
#[derive(Debug)]
pub struct TimetableNode {
    id: String,
    kind: TimetableNodeKind,
    input_edges: Vec<usize>,
    output_edges: Vec<usize>,
    predecessors: Vec<usize>,
    successors: Vec<usize>,
}

impl TimetableNode {
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

    /// Shared hardware-independent program, present on every compute node.
    #[must_use]
    pub fn program(&self) -> Option<&Rc<PreparedProgram>> {
        match &self.kind {
            TimetableNodeKind::Compute { program, .. } => Some(program),
            TimetableNodeKind::Tensor(_) => None,
        }
    }

    #[must_use]
    pub fn is_compute(&self) -> bool {
        matches!(self.kind, TimetableNodeKind::Compute { .. })
    }

    #[must_use]
    pub fn tensor(&self) -> Option<&Tensor> {
        match &self.kind {
            TimetableNodeKind::Compute { .. } => None,
            TimetableNodeKind::Tensor(tensor) => Some(tensor),
        }
    }

    #[must_use]
    pub fn input_edges(&self) -> &[usize] {
        &self.input_edges
    }

    #[must_use]
    pub fn output_edges(&self) -> &[usize] {
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
enum TimetableNodeKind {
    Compute {
        program: Rc<PreparedProgram>,
        pe: Option<String>,
        views: ComputeTensorViews,
    },
    Tensor(Tensor),
}

fn resolve_views(
    id: &str,
    names: &[String],
    mut views: BTreeMap<String, TensorViewSection>,
    direction: EdgeDirection,
) -> Result<Vec<Option<TensorViewSection>>, SimError> {
    let resolved = names.iter().map(|name| views.remove(name)).collect();
    if let Some(name) = views.keys().next() {
        return sim_error!("Node '{id}' has unknown {direction} view port '{name}'");
    }
    Ok(resolved)
}

struct ComputeViewSections {
    inputs: Vec<Option<TensorViewSection>>,
    outputs: Vec<Option<TensorViewSection>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeTensorDirection {
    Input,
    Output,
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
