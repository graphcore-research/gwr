// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::program::syntax::{ProgramSpec, TileSpec};
use gwr_models::processing_element::program::tile::plan_tiles;
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::task::ProgramTaskConfig;
use gwr_models::processing_element::tensors::{Tensor, TensorView};

use crate::program_compose::ProgramCatalog;
use crate::timetable_file::{
    EdgeKind, NodeSection, TensorViewSection, TimetableFile, parse_edge_end,
};

mod accesses;

/// A timetable whose nodes, edges, tensors, and views have been resolved and
/// validated.
#[derive(Debug)]
pub struct TimetableGraph {
    atomic_effects: Option<crate::atomic::AdmittedEffects>,
    nodes: Vec<TimetableNode>,
    edges: Vec<TimetableEdge>,
    topological_order: Vec<usize>,
    topological_positions: Vec<usize>,
}

impl TimetableGraph {
    #[expect(
        clippy::too_many_lines,
        reason = "Construct and validate graph indices in one ordered initialization pass."
    )]
    pub(crate) fn build(file: TimetableFile) -> Result<Self, SimError> {
        use ComputeTensorDirection::{Input, Output};
        let atomic_spec = file.atomic_effects.clone();

        let node_indices = node_indices(&file.nodes)?;
        let mut programs = ProgramCatalog::new(&file.programs);
        let mut nodes = Vec::with_capacity(file.nodes.len());
        let mut view_sections = Vec::with_capacity(file.nodes.len());
        for section in file.nodes {
            let (node, views) = TimetableNode::new(section, &mut programs)?;
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

            if section.kind != EdgeKind::Data {
                if section.kind == EdgeKind::Dependency {
                    nodes[from_node].successors.push(to_node);
                    nodes[to_node].predecessors.push(from_node);
                }
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
        let mut graph = Self {
            atomic_effects: None,
            nodes,
            edges,
            topological_order,
            topological_positions,
        };
        accesses::validate(&graph)?;
        graph.validate_programs()?;
        if let Some(spec) = atomic_spec {
            graph.atomic_effects = Some(
                crate::atomic::admit(
                    &graph,
                    &spec.memories,
                    &spec.bindings,
                    &spec.claims,
                    &spec.groups,
                )?
                .with_streams(&graph, &spec.streams)?,
            );
        }
        Ok(graph)
    }

    #[must_use]
    pub fn atomic_effects(&self) -> Option<&crate::atomic::AdmittedEffects> {
        self.atomic_effects.as_ref()
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

    fn validate_programs(&self) -> SimResult {
        for (node_index, node) in self.nodes.iter().enumerate() {
            if node.is_transfer() {
                let views = self.compute_views(node_index).unwrap();
                let source = views
                    .inputs()
                    .first()
                    .and_then(Option::as_ref)
                    .ok_or_else(|| SimError(format!("Transfer '{}' has no source", node.id())))?;
                let destination = views
                    .outputs()
                    .first()
                    .and_then(Option::as_ref)
                    .ok_or_else(|| {
                        SimError(format!("Transfer '{}' has no destination", node.id()))
                    })?;
                gwr_models::transfer::TransferTaskConfig {
                    id: node.id().into(),
                    source: source.clone(),
                    destination: destination.clone(),
                    buffer_bytes: node.transfer_buffer_bytes().unwrap(),
                }
                .validate()?;
                continue;
            }
            if !node.is_compute() {
                continue;
            }
            let Some(views) = self.compute_views(node_index) else {
                return sim_error!("Node '{}' has no compute views", node.id());
            };
            if let Some(program) = node.program() {
                let bindings = RegionBindings {
                    inputs: views
                        .inputs()
                        .iter()
                        .cloned()
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            SimError(format!(
                                "Compute node '{}' has an unbound program input",
                                node.id()
                            ))
                        })?,
                    outputs: views
                        .outputs()
                        .iter()
                        .cloned()
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            SimError(format!(
                                "Compute node '{}' has an unbound program output",
                                node.id()
                            ))
                        })?,
                };
                let region = validate_program(program, &bindings)
                    .map_err(|error| SimError(format!("Compute node '{}': {error}", node.id())))?;
                plan_tiles(&region, node.tiling())
                    .map_err(|error| SimError(format!("Compute node '{}': {error}", node.id())))?;
            }
        }
        Ok(())
    }
    #[must_use]
    pub fn compute_views(&self, node_index: usize) -> Option<ComputeTensorViews> {
        let node = self.nodes.get(node_index)?;
        node.is_task().then_some(())?;
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

    /// Build immutable compute-task inputs for engine-free analytical models.
    /// The graph has already validated task programs, tensor bindings and
    /// dependency structure; platform-specific address/capability checks are
    /// performed by the analyzer consuming this snapshot.
    pub fn analysis_activities(&self) -> Result<Vec<AnalysisActivity>, SimError> {
        let mut activities = Vec::new();
        for (node_index, node) in self.nodes.iter().enumerate() {
            if !node.is_compute() && !node.is_transfer() {
                continue;
            }
            let pe_name = node
                .pe()
                .ok_or_else(|| SimError(format!("Task node '{}' has no PE", node.id())))?;
            let views = self.compute_views(node_index).ok_or_else(|| {
                SimError(format!("Task node '{}' has no tensor views", node.id()))
            })?;
            let (task, transfer_views) = if node.is_transfer() {
                let source = views.inputs.first().and_then(Clone::clone).ok_or_else(|| {
                    SimError(format!("transfer {} has unbound source", node.id()))
                })?;
                let destination =
                    views
                        .outputs
                        .first()
                        .and_then(Clone::clone)
                        .ok_or_else(|| {
                            SimError(format!("transfer {} has unbound destination", node.id()))
                        })?;
                (None, Some((source, destination)))
            } else {
                let task = ProgramTaskConfig {
                    id: node.id().to_owned(),
                    program: node.program().unwrap().clone(),
                    tiling: node.tiling().cloned(),
                    pipeline: node.pipeline().cloned(),
                    inputs: views
                        .inputs
                        .into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            SimError(format!("program {} has unbound input", node.id()))
                        })?,
                    outputs: views
                        .outputs
                        .into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            SimError(format!("program {} has unbound output", node.id()))
                        })?,
                };
                (Some(task), None)
            };
            let mut predecessors = std::collections::BTreeSet::new();
            for &predecessor in node.predecessors() {
                let predecessor_node = &self.nodes[predecessor];
                if predecessor_node.is_compute() || predecessor_node.is_transfer() {
                    predecessors.insert(predecessor);
                } else if predecessor_node.tensor().is_some() {
                    for &producer in predecessor_node.predecessors() {
                        if self.nodes[producer].is_compute() || self.nodes[producer].is_transfer() {
                            predecessors.insert(producer);
                        }
                    }
                }
            }
            activities.push(AnalysisActivity {
                node_index,
                id: node.id().to_owned(),
                pe_name: pe_name.to_owned(),
                task,
                transfer_views,
                predecessors: predecessors.into_iter().collect(),
            });
        }
        Ok(activities)
    }
}

/// A validated compute or transfer node and its incoming task dependencies.
#[derive(Clone, Debug)]
pub struct AnalysisActivity {
    pub node_index: usize,
    pub id: String,
    pub pe_name: String,
    pub task: Option<ProgramTaskConfig>,
    pub transfer_views: Option<(TensorView, TensorView)>,
    pub predecessors: Vec<usize>,
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

fn resolve_edge_index(
    node: &TimetableNode,
    direction: EdgeDirection,
    requested_index: Option<usize>,
    used: &mut EdgeIndexUsage,
) -> Result<usize, SimError> {
    used.claim(node, direction, requested_index)
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
            EdgeDirection::Input => node.is_task().then_some(node.input_edges.len()),
            EdgeDirection::Output => node.is_task().then_some(node.output_edges.len()),
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

#[derive(Clone, Copy)]
enum EdgeDirection {
    Input,
    Output,
}

impl fmt::Display for EdgeDirection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input => formatter.write_str("input"),
            Self::Output => formatter.write_str("output"),
        }
    }
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

#[derive(Debug)]
pub struct TimetableNode {
    id: String,
    kind: TimetableNodeKind,
    input_edges: Vec<Option<usize>>,
    output_edges: Vec<Option<usize>>,
    predecessors: Vec<usize>,
    successors: Vec<usize>,
}

impl TimetableNode {
    fn new(
        section: NodeSection,
        programs: &mut ProgramCatalog<'_>,
    ) -> Result<(Self, Option<ComputeViewSections>), SimError> {
        let (id, kind, views, num_inputs, num_outputs) = match section {
            NodeSection::Compute {
                id,
                program,
                tiling,
                pipeline,
                pe,
                input_views,
                output_views,
            } => {
                if let Some(request) = &pipeline {
                    request.validate()?;
                }
                let program = programs.resolve(program, &id)?;
                let input_views = input_views.unwrap_or_else(|| vec![None; program.inputs.len()]);
                let output_views =
                    output_views.unwrap_or_else(|| vec![None; program.outputs.len()]);
                let num_inputs = input_views.len();
                let num_outputs = output_views.len();
                (
                    id,
                    TimetableNodeKind::Compute {
                        program,
                        tiling,
                        pipeline,
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
            NodeSection::Transfer {
                id,
                pe,
                buffer_bytes,
                input_views,
                output_views,
            } => {
                let inputs = input_views.unwrap_or_else(|| vec![None]);
                let outputs = output_views.unwrap_or_else(|| vec![None]);
                if inputs.len() != 1 || outputs.len() != 1 {
                    return sim_error!(
                        "Transfer '{id}' requires exactly one input and output view"
                    );
                }
                (
                    id,
                    TimetableNodeKind::Transfer { pe, buffer_bytes },
                    Some(ComputeViewSections { inputs, outputs }),
                    1,
                    1,
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
            TimetableNodeKind::Compute { pe, .. } | TimetableNodeKind::Transfer { pe, .. } => {
                pe.as_deref()
            }
            TimetableNodeKind::Tensor(_) => None,
        }
    }

    #[must_use]
    pub fn program(&self) -> Option<&ProgramSpec> {
        match &self.kind {
            TimetableNodeKind::Compute { program, .. } => Some(program),
            TimetableNodeKind::Tensor(_) | TimetableNodeKind::Transfer { .. } => None,
        }
    }

    #[must_use]
    /// Return the explicit bounded block schedule, if selected.
    pub fn pipeline(
        &self,
    ) -> Option<&gwr_models::processing_element::program::pipeline::BlockPipelineSpec> {
        match &self.kind {
            TimetableNodeKind::Compute { pipeline, .. } => pipeline.as_ref(),
            _ => None,
        }
    }

    #[must_use]
    pub fn tiling(&self) -> Option<&TileSpec> {
        match &self.kind {
            TimetableNodeKind::Compute { tiling, .. } => tiling.as_ref(),
            TimetableNodeKind::Tensor(_) | TimetableNodeKind::Transfer { .. } => None,
        }
    }

    #[must_use]
    pub fn is_compute(&self) -> bool {
        matches!(self.kind, TimetableNodeKind::Compute { .. })
    }

    /// Whether this node executes on a processing element.
    #[must_use]
    pub fn is_task(&self) -> bool {
        self.tensor().is_none()
    }

    #[must_use]
    pub fn is_transfer(&self) -> bool {
        matches!(self.kind, TimetableNodeKind::Transfer { .. })
    }

    #[must_use]
    pub fn transfer_buffer_bytes(&self) -> Option<usize> {
        match self.kind {
            TimetableNodeKind::Transfer { buffer_bytes, .. } => Some(buffer_bytes),
            _ => None,
        }
    }

    #[must_use]
    pub fn tensor(&self) -> Option<&Tensor> {
        match &self.kind {
            TimetableNodeKind::Compute { .. } | TimetableNodeKind::Transfer { .. } => None,
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
enum TimetableNodeKind {
    Compute {
        program: ProgramSpec,
        tiling: Option<TileSpec>,
        pipeline: Option<gwr_models::processing_element::program::pipeline::BlockPipelineSpec>,
        pe: Option<String>,
    },
    Transfer {
        pe: Option<String>,
        buffer_bytes: usize,
    },
    Tensor(Tensor),
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
