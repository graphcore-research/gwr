// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A library for defining a [Timetable] that can be run on a [Platform].
//!
//! `gwr-timetable` provides a front-end utility for running timetables. For
//! example:
//!   cargo run --bin gwr-timetable --
//!     --platform gwr-platform/examples/platform.yaml
//!     --timetable gwr-timetable/examples/small.yaml
//!     --stdout --stdout-level debug
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::sim_error;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::EntityGet;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::operators::{MachineOp, MachineOps, Tensor, TensorView};
use gwr_models::processing_element::task::{
    ComputeOp, ComputeTaskConfig, MemoryOp, MemoryTaskConfig, Task,
};
use gwr_platform::Platform;
use gwr_track::entity::Entity;
use gwr_track::{debug, info, trace};

pub mod analysis;
pub mod mermaid;
pub mod timetable_file;
pub mod types;
pub use analysis::ComputeNodeAnalysis;
use timetable_file::{NodeSection, TimetableFile};
use types::Node;

use crate::mermaid::{MermaidNodeStatus, render_mermaid_from_parts};
use crate::timetable_file::{
    EdgeSection, MemoryConfigSection, TensorConfigSection, TensorViewSection, dtype_num_bytes,
};

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
        // When the view is not provided it means we are simply using the entire Tensor
        // so it is ok
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

    for (i, ((offset, size), tensor_dim)) in view
        .offsets
        .iter()
        .zip(view.shape.iter())
        .zip(tensor_config.shape.iter())
        .enumerate()
    {
        if (offset + size) > *tensor_dim {
            return sim_error!(
                "{direction} view on node '{}' is out of range in dim {i}: offset {offset} + size {size} > {tensor_dim}",
                node_id,
            );
        }
    }

    Ok(())
}

fn tensor_view_offset(
    tensor_config: &TensorConfigSection,
    view: Option<&TensorViewSection>,
) -> usize {
    match view {
        Some(view) => view
            .offsets
            .iter()
            .enumerate()
            .map(|(i, offset)| {
                let stride: usize = tensor_config.shape[(i + 1)..].iter().product();
                offset * stride
            })
            .sum(),
        None => 0,
    }
}

fn tensor_view_num_elements(
    tensor_config: &TensorConfigSection,
    view: Option<&TensorViewSection>,
) -> usize {
    match view {
        Some(view) => view.num_elements(),
        None => tensor_config.num_elements(),
    }
}

#[derive(EntityGet)]
pub struct Timetable {
    entity: Rc<Entity>,
    platform: Rc<Platform>,
    nodes: Vec<Node>,
    edges: Vec<EdgeSection>,
    completed_node_indices: RefCell<HashSet<usize>>,
    active_node_indices: RefCell<HashSet<usize>>,
    // Use BTreeSet for the cases where we iterate over the set as they have
    // deterministic iteration order.
    nodes_per_pe: RefCell<HashMap<usize, BTreeSet<usize>>>,
    ready_nodes_changed: Repeated<()>,
}

impl fmt::Debug for Timetable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timetable")
            .field("entity", &self.entity)
            .finish()
    }
}

/// Make an edge connection by updating the edge indices of a given node.
///
/// If the edge_idx is given then ensure the vector of edges is large enough
/// and assign. Otherwise, simply find an unassigned index or extend the vector
/// in order to record the edge.
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
    } else {
        let mut inserted = false;
        for edge_idx in edge_indices.iter_mut() {
            if edge_idx.is_none() {
                *edge_idx = Some(node_idx);
                inserted = true;
                break;
            }
        }
        if !inserted {
            edge_indices.push(Some(node_idx));
        }
    }
    Ok(())
}

type InOutTensorViews = (Vec<Option<TensorView>>, Vec<Option<TensorView>>);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TimetableStats {
    pub total_load_bytes: usize,
    pub total_store_bytes: usize,
    pub num_compute_nodes: usize,
    pub num_tensor_nodes: usize,
    pub num_memory_nodes: usize,
    pub machine_ops: MachineOps,
    pub total_machine_ops: usize,
}

#[must_use]
pub fn format_machine_ops(machine_ops: &MachineOps) -> String {
    let counts = MachineOp::ALL
        .into_iter()
        .filter_map(|machine_op| {
            machine_ops
                .get(&machine_op)
                .map(|count| format!("{machine_op}={count}"))
        })
        .collect::<Vec<_>>();

    if counts.is_empty() {
        "none".to_string()
    } else {
        counts.join(", ")
    }
}

fn accumulate_machine_ops(total: &mut MachineOps, addend: &MachineOps) {
    for machine_op in MachineOp::ALL {
        if let Some(count) = addend.get(&machine_op) {
            total.add_op(machine_op, *count);
        }
    }
}

impl Timetable {
    /// Create a Timetable from a TimetableFile and validate it
    ///
    /// Build any helper structures required for quick accesses. This includes:
    ///  - a map of Nodes that are mapped to each Processing Element (PE)
    ///  - new nodes that wrap the contents of the file but also have the edge
    ///    links
    pub fn new(
        parent: &Rc<Entity>,
        mut timetable_file: TimetableFile,
        platform: &Rc<Platform>,
    ) -> Result<Self, SimError> {
        timetable_file.validate(platform)?;

        let entity = Rc::new(Entity::new(parent, "timetable"));
        let mut node_idx_by_id = HashMap::new();
        let mut nodes_per_pe = HashMap::new();
        let mut nodes = Vec::with_capacity(timetable_file.nodes.len());

        for (i, node_section) in timetable_file.nodes.drain(..).enumerate() {
            let (id, pe) = node_section.id_pe();
            node_idx_by_id.insert(id.clone(), i);

            if let Some(pe_id) = &pe {
                let pe_idx = platform.pe_idx_from_name(pe_id)?;
                nodes_per_pe
                    .entry(pe_idx)
                    .or_insert_with(BTreeSet::new)
                    .insert(i);
            }

            nodes.push(Node {
                node_section,
                inputs: Vec::new(),
                outputs: Vec::new(),
            });
        }

        // Wire up the new node inputs/outputs to build the graph connectivity
        for edge_section in &timetable_file.edges {
            // Note: we have validated the edges so we can just unwrap()
            let (from_node_id, from_edge_idx) = edge_section.from_node_and_edge()?;
            let from_node_idx = node_idx_by_id.get(from_node_id).unwrap();
            let (to_node_id, to_edge_idx) = edge_section.to_node_and_edge()?;
            let to_node_idx = node_idx_by_id.get(to_node_id).unwrap();

            update_edge_indices(*from_node_idx, to_edge_idx, &mut nodes[*to_node_idx].inputs)
                .map_err(|err| {
                    SimError(format!(
                        "Node {from_node_idx} '{}': {err}",
                        &nodes[*from_node_idx].node_section.id()
                    ))
                })?;
            update_edge_indices(
                *to_node_idx,
                from_edge_idx,
                &mut nodes[*from_node_idx].outputs,
            )
            .map_err(|err| {
                SimError(format!(
                    "Node {to_node_idx} '{}': {err}",
                    &nodes[*to_node_idx].node_section.id()
                ))
            })?;
        }

        let timetable = Self {
            entity,
            nodes,
            edges: timetable_file.edges,
            platform: platform.clone(),
            completed_node_indices: RefCell::new(HashSet::new()),
            active_node_indices: RefCell::new(HashSet::new()),
            nodes_per_pe: RefCell::new(nodes_per_pe),
            ready_nodes_changed: Repeated::new(()),
        };

        timetable.validate()?;

        timetable.update_complete_tensors();

        Ok(timetable)
    }

    fn make_tensor_view(
        tensor_config: &TensorConfigSection,
        view: Option<&TensorViewSection>,
    ) -> Result<TensorView, SimError> {
        let tensor = Tensor::new(
            &tensor_config.shape,
            &tensor_config.dtype,
            tensor_config.addr,
        );
        match view {
            Some(view) => Ok(TensorView::new(tensor, &view.shape, &view.offsets)),
            None => Ok(TensorView::new_full(tensor)),
        }
    }

    fn validate(&self) -> SimResult {
        for node in &self.nodes {
            match &node.node_section {
                NodeSection::Memory { id, op, config, .. } => match op {
                    MemoryOp::Load => {
                        self.validate_load_node(id, node, config)?;
                    }
                    MemoryOp::Store => {
                        self.validate_store_node(id, node, config)?;
                    }
                },
                NodeSection::Compute {
                    id,
                    input_views,
                    output_views,
                    ..
                } => {
                    self.validate_compute_node(node, id, input_views, output_views)?;
                }
                NodeSection::Tensor { .. } => {
                    // Nothing for now
                }
            }
        }

        Ok(())
    }

    /// Given a Node, return the input Tensor config for a Memory Load and the
    /// output Tensor config for a Memory Store. In all other cases returns
    /// None.
    fn get_tensor_node_config(&self, node: &Node) -> Option<&TensorConfigSection> {
        node.get_memory_tensor_node_idx().map(|node_idx| {
            let node = &self.nodes[node_idx];
            if let NodeSection::Tensor { config, .. } = &node.node_section {
                Some(config)
            } else {
                None
            }
        })?
    }

    fn validate_compute_node(
        &self,
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
            let tensor_node = &self.nodes[*tensor_idx];
            let NodeSection::Tensor { config, .. } = &tensor_node.node_section else {
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
            let tensor_node = &self.nodes[*tensor_idx];
            let NodeSection::Tensor { config, .. } = &tensor_node.node_section else {
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
        &self,
        id: &str,
        load_node: &Node,
        load_config: &MemoryConfigSection,
    ) -> SimResult {
        if load_node.inputs.len() != 1 {
            return sim_error!(
                "{} edges connect into Load node '{id}'",
                load_node.inputs.len(),
            );
        }

        let Some(config) = self.get_tensor_node_config(load_node) else {
            return sim_error!("Load node '{id}' not connected from Tensor node",);
        };
        validate_access_in_range(id, "Load", load_config, config)
    }

    fn validate_store_node(
        &self,
        id: &str,
        store_node: &Node,
        store_config: &MemoryConfigSection,
    ) -> SimResult {
        if store_node.outputs.len() != 1 {
            return sim_error!(
                "{} edges connect from Store node '{id}'",
                store_node.outputs.len(),
            );
        }

        let Some(config) = self.get_tensor_node_config(store_node) else {
            return sim_error!("Store node '{id}' not connected to Tensor node");
        };
        validate_access_in_range(id, "Store", store_config, config)
    }

    /// Check a given tensor index and move it if it is now complete
    fn update_complete_tensor(&self, tensor_idx: usize) {
        let mut completed_node_indices = self.completed_node_indices.borrow_mut();
        let tensor_node = &self.nodes[tensor_idx];

        // Look for an input node that is not complete
        for idx in tensor_node.inputs.iter().flatten() {
            if !completed_node_indices.contains(idx) {
                return;
            }
        }

        // No active inputs remain, this is now complete
        self.active_node_indices.borrow_mut().remove(&tensor_idx);
        completed_node_indices.insert(tensor_idx);
    }

    /// Iterate across all active tensors and move those that are now complete
    fn update_complete_tensors(&self) {
        for (idx, node) in self.nodes.iter().enumerate() {
            if let NodeSection::Tensor { .. } = node.node_section {
                self.update_complete_tensor(idx);
            }
        }
    }

    pub fn total_tasks(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn num_graph_nodes_completed(&self) -> usize {
        self.completed_node_indices.borrow().len()
    }

    fn memory_access_address_num_bytes(
        &self,
        memory_node: &Node,
        config: &MemoryConfigSection,
    ) -> (u64, usize) {
        // Note we assume that the graph has been validated so that we can simply unwrap
        // the result
        let tensor_config = self.get_tensor_node_config(memory_node).unwrap();
        let offset_num_elements = tensor_view_offset(tensor_config, config.view.as_ref());
        let view_num_elements = tensor_view_num_elements(tensor_config, config.view.as_ref());
        let address =
            tensor_config.addr + dtype_num_bytes(&tensor_config.dtype, offset_num_elements) as u64;
        let num_bytes = dtype_num_bytes(&tensor_config.dtype, view_num_elements);
        (address, num_bytes)
    }

    pub fn get_input_output_tensors(&self, node_idx: usize) -> Result<InOutTensorViews, SimError> {
        let node = &self.nodes[node_idx];
        let NodeSection::Compute {
            id,
            input_views,
            output_views,
            ..
        } = &node.node_section
        else {
            return sim_error!("node {} is not a compute node", node.node_section.id());
        };

        let mut input_tensors = Vec::new();
        for (input_idx, input_node_idx) in node.inputs.iter().enumerate() {
            if let Some(idx) = input_node_idx {
                let tensor_node = &self.nodes[*idx];
                let NodeSection::Tensor { config, .. } = &tensor_node.node_section else {
                    return sim_error!(
                        "{}: input {} is not connected from a Tensor node",
                        id,
                        input_idx
                    );
                };
                input_tensors.push(Some(Self::make_tensor_view(
                    config,
                    input_views[input_idx].as_ref(),
                )?));
            } else {
                input_tensors.push(None);
            }
        }

        let mut output_tensors = Vec::new();
        for (output_idx, output_node_idx) in node.outputs.iter().enumerate() {
            if let Some(idx) = output_node_idx {
                let tensor_node = &self.nodes[*idx];
                let NodeSection::Tensor { config, .. } = &tensor_node.node_section else {
                    return sim_error!(
                        "{}: output {} is not connected to a Tensor node",
                        id,
                        output_idx
                    );
                };
                output_tensors.push(Some(Self::make_tensor_view(
                    config,
                    output_views[output_idx].as_ref(),
                )?));
            } else {
                output_tensors.push(None);
            }
        }

        Ok((input_tensors, output_tensors))
    }

    pub fn check_tasks_complete(&self) -> SimResult {
        let num_active = self.active_node_indices.borrow().len();
        if num_active != 0 {
            return sim_error!("{num_active} tasks still active");
        }

        let num_completed = self.completed_node_indices.borrow().len();
        let num_tasks = self.nodes.len();
        if num_completed != num_tasks {
            return sim_error!(
                "{num_completed} tasks completed out of a total of {num_tasks} tasks."
            );
        }

        Ok(())
    }

    pub fn stats(&self) -> Result<TimetableStats, SimError> {
        let mut stats = TimetableStats::default();
        for (idx, node) in self.nodes.iter().enumerate() {
            match &node.node_section {
                NodeSection::Memory { op, config, .. } => {
                    let (_, num_bytes) = self.memory_access_address_num_bytes(node, config);
                    match op {
                        MemoryOp::Load => stats.total_load_bytes += num_bytes,
                        MemoryOp::Store => stats.total_store_bytes += num_bytes,
                    }
                    stats.num_memory_nodes += 1;
                }
                NodeSection::Compute { op, .. } => {
                    let (inputs, outputs) = self.get_input_output_tensors(idx)?;
                    for input_view in inputs.iter().flatten() {
                        stats.total_load_bytes += input_view.num_bytes();
                    }
                    for output_view in outputs.iter().flatten() {
                        stats.total_store_bytes += output_view.num_bytes();
                    }
                    let machine_ops = op.compute_flops(&inputs, &outputs)?;
                    accumulate_machine_ops(&mut stats.machine_ops, &machine_ops);
                    stats.total_machine_ops += machine_ops.total_flops();
                    stats.num_compute_nodes += 1;
                }
                NodeSection::Tensor { .. } => stats.num_tensor_nodes += 1,
            }
        }

        Ok(stats)
    }

    pub fn compute_node_analyses(&self) -> Result<Vec<ComputeNodeAnalysis>, SimError> {
        let mut analyses = Vec::new();

        for (idx, node) in self.nodes.iter().enumerate() {
            let NodeSection::Compute { id, pe, op, .. } = &node.node_section else {
                continue;
            };

            let (inputs, outputs) = self.get_input_output_tensors(idx)?;
            let input_bytes = inputs.iter().flatten().map(TensorView::num_bytes).sum();
            let output_bytes = outputs.iter().flatten().map(TensorView::num_bytes).sum();
            let machine_ops = op.compute_flops(&inputs, &outputs)?;
            let flops = machine_ops.total_flops();
            let mut bytes_by_memory = BTreeMap::new();
            let mut tensor_access_addrs = Vec::new();
            for view in inputs.iter().chain(outputs.iter()).flatten() {
                let addr = view.tensor().addr();
                let memory_name = self.platform.memory_name_for_address(addr)?;
                *bytes_by_memory.entry(memory_name).or_insert(0) += view.num_bytes();
                tensor_access_addrs.push(addr);
            }

            let mut predecessor_compute_node_indices = BTreeSet::new();
            for tensor_node_idx in node.inputs.iter().flatten() {
                let tensor_node = &self.nodes[*tensor_node_idx];
                for producer_node_idx in tensor_node.inputs.iter().flatten() {
                    if matches!(
                        self.nodes[*producer_node_idx].node_section,
                        NodeSection::Compute { .. }
                    ) {
                        predecessor_compute_node_indices.insert(*producer_node_idx);
                    }
                }
            }

            let predecessor_compute_node_indices = predecessor_compute_node_indices
                .into_iter()
                .collect::<Vec<_>>();
            let predecessor_compute_node_ids = predecessor_compute_node_indices
                .iter()
                .map(|producer_node_idx| self.nodes[*producer_node_idx].node_section.id().clone())
                .collect::<Vec<_>>();

            analyses.push(ComputeNodeAnalysis {
                node_idx: idx,
                id: id.clone(),
                pe_name: pe.clone(),
                op: op.clone(),
                machine_ops,
                flops,
                input_bytes,
                output_bytes,
                bytes_by_memory,
                tensor_access_addrs,
                predecessor_compute_node_indices,
                predecessor_compute_node_ids,
            });
        }

        Ok(analyses)
    }

    pub fn dump_stats(&self) -> SimResult {
        let stats = self.stats()?;

        info!(self.entity ; "Timetable:");
        info!(self.entity ;
            "  {} compute nodes, {} tensor nodes, {} memory nodes",
            stats.num_compute_nodes,
            stats.num_tensor_nodes,
            stats.num_memory_nodes
        );
        info!(self.entity ;
            "  loads {} bytes, stores {} bytes",
            stats.total_load_bytes,
            stats.total_store_bytes
        );
        info!(self.entity ;
            "  machine ops: {} ({})",
            stats.total_machine_ops,
            format_machine_ops(&stats.machine_ops)
        );

        Ok(())
    }

    /// Create map of node ID to status for rendering
    #[must_use]
    pub fn mermaid_node_statuses(&self) -> HashMap<String, MermaidNodeStatus> {
        let completed = self.completed_node_indices.borrow();
        let active = self.active_node_indices.borrow();

        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, node)| match &node.node_section {
                NodeSection::Compute { id, .. } | NodeSection::Tensor { id, .. } => {
                    let status = if completed.contains(&idx) {
                        MermaidNodeStatus::Complete
                    } else if active.contains(&idx) {
                        MermaidNodeStatus::Active
                    } else {
                        MermaidNodeStatus::Pending
                    };
                    Some((id.clone(), status))
                }
                NodeSection::Memory { .. } => None,
            })
            .collect()
    }

    /// Render a mermaid of the current status of the Timetable
    #[must_use]
    pub fn render_mermaid(&self) -> String {
        // Need to rebuild a Vec of the NodeSection as that is what the mermaid renderer
        // uses
        let nodes: Vec<NodeSection> = self
            .nodes
            .iter()
            .map(|node| node.node_section.clone())
            .collect();
        render_mermaid_from_parts(&nodes, &self.edges, &self.mermaid_node_statuses())
    }
}

fn build_compute_task(
    id: &str,
    op: ComputeOp,
    inputs: Vec<Option<TensorView>>,
    outputs: Vec<Option<TensorView>>,
) -> Task {
    Task::ComputeTask {
        config: ComputeTaskConfig {
            id: id.to_string(),
            op,
            inputs,
            outputs,
        },
    }
}

fn build_memory_task(id: &str, op: MemoryOp, addr: u64, num_bytes: usize) -> Task {
    Task::MemoryTask {
        config: MemoryTaskConfig {
            id: id.to_string(),
            op,
            addr,
            num_bytes,
        },
    }
}

#[async_trait(?Send)]
impl Dispatch for Timetable {
    fn task_by_id(&self, task_idx: usize) -> Result<Task, SimError> {
        let node = &self.nodes[task_idx];
        match &node.node_section {
            NodeSection::Compute { id, op, .. } => {
                let (inputs, outputs) = self.get_input_output_tensors(task_idx)?;
                Ok(build_compute_task(id, op.clone(), inputs, outputs))
            }
            NodeSection::Memory { id, op, config, .. } => {
                let (address, num_bytes) = self.memory_access_address_num_bytes(node, config);
                Ok(build_memory_task(id, *op, address, num_bytes))
            }
            NodeSection::Tensor { .. } => {
                sim_error!("Task Index {task_idx} refers to a Tensor node")
            }
        }
    }

    fn set_task_active(&self, node_idx: usize) -> SimResult {
        debug!(self.entity; "task{node_idx}: active");
        self.active_node_indices.borrow_mut().insert(node_idx);
        self.ready_nodes_changed.notify();
        Ok(())
    }

    fn set_task_completed(&self, node_idx: usize) -> SimResult {
        debug!(self.entity; "task{node_idx}: completed");

        let node = &self.nodes[node_idx];
        let pe = node.node_section.pe();
        if let Some(pe) = pe {
            let pe_idx = self.platform.pe_idx_from_name(pe)?;
            let mut nodes_per_pe_guard = self.nodes_per_pe.borrow_mut();
            nodes_per_pe_guard
                .get_mut(&pe_idx)
                .unwrap()
                .remove(&node_idx);
        }
        self.active_node_indices.borrow_mut().remove(&node_idx);
        self.completed_node_indices.borrow_mut().insert(node_idx);

        match node.node_section {
            NodeSection::Compute { .. } => {
                for tensor_node_idx in node.outputs.iter().flatten() {
                    self.update_complete_tensor(*tensor_node_idx);
                }
            }
            NodeSection::Memory { op, .. } => {
                if let MemoryOp::Store = op {
                    // Only stores are completing their output tensors
                    let tensor_node_idx = node.get_output_tensor_node_idx().unwrap();
                    self.update_complete_tensor(tensor_node_idx);
                }
            }
            NodeSection::Tensor { .. } => {}
        }

        self.ready_nodes_changed.notify();
        Ok(())
    }

    fn ready_task_indices(&self, pe_id: &str) -> Result<(bool, Vec<usize>), SimError> {
        trace!(self.entity ; "ready_node_indices for {pe_id}");
        let mut pe_done = true;
        let completed_guard = self.completed_node_indices.borrow();
        let active_guard = self.active_node_indices.borrow();
        let mut ready_node_indices = Vec::new();
        let pe_idx = self.platform.pe_idx_from_name(pe_id)?;
        let nodes_per_pe_guard = self.nodes_per_pe.borrow();
        if let Some(nodes) = nodes_per_pe_guard.get(&pe_idx) {
            for node_idx in nodes {
                trace!(self.entity ; "ready? {node_idx}");
                if active_guard.contains(node_idx) {
                    trace!(self.entity ; "{node_idx} active");
                    continue;
                }
                if completed_guard.contains(node_idx) {
                    trace!(self.entity ; "{node_idx} complete");
                    continue;
                }
                pe_done = false;

                let mut ready = true;

                let to_node = &self.nodes[*node_idx];
                let to_pe = to_node.node_section.pe();
                for from_node_idx in &to_node.inputs {
                    if from_node_idx.is_none() || to_pe.is_none() {
                        continue;
                    }

                    let from_node_idx = from_node_idx.as_ref().unwrap();
                    let to_pe_id = to_pe.as_ref().unwrap();
                    trace!(self.entity ; "-> {to_pe_id}");
                    let to_pe_idx = self.platform.pe_idx_from_name(to_pe_id)?;
                    trace!(self.entity ; "idx {to_pe_idx}");
                    if to_pe_idx == pe_idx && !completed_guard.contains(from_node_idx) {
                        trace!(self.entity ; "{node_idx} not ready");
                        ready = false;
                        break;
                    }
                }

                if ready {
                    trace!(self.entity ; "{node_idx} ready");
                    ready_node_indices.push(*node_idx);
                }
            }
        }
        debug!(self.entity; "PE {pe_id}: done: {pe_done}, ready indices: {ready_node_indices:?}");
        Ok((pe_done, ready_node_indices))
    }

    async fn wait_for_change(&self) {
        self.ready_nodes_changed.listen().await;
    }

    fn total_tasks_for_pe(&self, pe_name: &str) -> usize {
        let mut num_nodes = 0;
        for node in &self.nodes {
            let pe = node.node_section.pe();
            if let Some(pe) = pe
                && pe == pe_name
            {
                num_nodes += 1;
            }
        }
        num_nodes
    }
}

#[cfg(test)]
mod tests {
    use gwr_models::processing_element::operators::dtype::DataType;

    use super::tensor_view_offset;
    use crate::timetable_file::{TensorConfigSection, TensorViewSection};

    fn tensor_config(shape: Vec<usize>) -> TensorConfigSection {
        TensorConfigSection {
            addr: 0,
            dtype: DataType::Fp16,
            shape,
        }
    }

    #[test]
    fn tensor_view_offset_none_view_is_zero() {
        let config = tensor_config(vec![3, 4, 5]);
        assert_eq!(tensor_view_offset(&config, None.as_ref()), 0);
    }

    #[test]
    fn tensor_view_offset_1d() {
        let config = tensor_config(vec![10]);
        let view = Some(TensorViewSection {
            offsets: vec![7],
            shape: vec![2],
        });
        assert_eq!(tensor_view_offset(&config, view.as_ref()), 7);
    }

    #[test]
    fn tensor_view_offset_2d_row_major() {
        let config = tensor_config(vec![4, 5]);
        let view = Some(TensorViewSection {
            offsets: vec![2, 3],
            shape: vec![1, 1],
        });
        assert_eq!(tensor_view_offset(&config, view.as_ref()), 13);
    }

    #[test]
    fn tensor_view_offset_3d_row_major() {
        let config = tensor_config(vec![3, 4, 5]);
        let view = Some(TensorViewSection {
            offsets: vec![1, 2, 3],
            shape: vec![1, 1, 1],
        });
        assert_eq!(tensor_view_offset(&config, view.as_ref()), 33);
    }
}
