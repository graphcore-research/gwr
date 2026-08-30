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
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::sim_error;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::EntityGet;
use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::operators::TensorView;
use gwr_models::processing_element::task::{ComputeOp, ComputeTaskConfig, Task};
use gwr_platform::Platform;
use gwr_track::entity::Entity;
use gwr_track::{debug, info, trace};

pub mod graph;
pub mod mermaid;
pub mod timetable_file;
pub use graph::{
    ComputeTensorDirection, ComputeTensorViews, EdgeEndpoint, TensorConnection, TimetableEdge,
    TimetableGraph, TimetableNode,
};

use crate::mermaid::{MermaidNodeStatus, render_mermaid};

#[derive(EntityGet)]
pub struct Timetable {
    entity: Rc<Entity>,
    platform: Rc<Platform>,
    graph: TimetableGraph,
    node_pe_indices: Vec<Option<usize>>,
    completed_node_indices: RefCell<HashSet<usize>>,
    active_node_indices: RefCell<HashSet<usize>>,
    // Use BTreeSet for the cases where we iterate over the set as they have
    // deterministic iteration order.
    nodes_per_pe: HashMap<usize, BTreeSet<usize>>,
    ready_nodes_per_pe: RefCell<HashMap<usize, BTreeSet<usize>>>,
    remaining_nodes_per_pe: RefCell<HashMap<usize, usize>>,
    unresolved_input_counts: RefCell<Vec<usize>>,
    ready_nodes_changed: Repeated<()>,
}

impl fmt::Debug for Timetable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timetable")
            .field("entity", &self.entity)
            .finish()
    }
}

impl Timetable {
    /// Create a runnable timetable from a validated graph.
    pub fn new(
        parent: &Rc<Entity>,
        graph: TimetableGraph,
        platform: &Rc<Platform>,
    ) -> Result<Self, SimError> {
        let entity = Rc::new(Entity::new(parent, "timetable"));
        let mut nodes_per_pe = HashMap::new();
        let mut node_pe_indices = Vec::with_capacity(graph.nodes().len());

        for (node_index, node) in graph.nodes().iter().enumerate() {
            let pe_idx = if let Some(pe) = node.pe() {
                let pe_idx = platform.pe_idx_from_name(pe).map_err(|_| {
                    SimError(format!(
                        "Node '{}' contains invalid PE ID '{pe}'",
                        node.id()
                    ))
                })?;
                nodes_per_pe
                    .entry(pe_idx)
                    .or_insert_with(BTreeSet::new)
                    .insert(node_index);
                Some(pe_idx)
            } else {
                None
            };
            node_pe_indices.push(pe_idx);
        }

        let timetable = Self {
            entity,
            graph,
            node_pe_indices,
            platform: platform.clone(),
            completed_node_indices: RefCell::new(HashSet::new()),
            active_node_indices: RefCell::new(HashSet::new()),
            nodes_per_pe,
            ready_nodes_per_pe: RefCell::new(HashMap::new()),
            remaining_nodes_per_pe: RefCell::new(HashMap::new()),
            unresolved_input_counts: RefCell::new(Vec::new()),
            ready_nodes_changed: Repeated::new(()),
        };

        timetable.update_complete_tensors();
        timetable.initialize_scheduler_state();

        Ok(timetable)
    }

    /// Check a given tensor index and move it if it is now complete.
    fn update_complete_tensor(&self, tensor_idx: usize) -> bool {
        let mut completed_node_indices = self.completed_node_indices.borrow_mut();
        if completed_node_indices.contains(&tensor_idx) {
            return false;
        }

        let tensor_node = &self.graph.nodes()[tensor_idx];

        // Look for an input node that is not complete
        for idx in tensor_node.predecessors() {
            if !completed_node_indices.contains(idx) {
                return false;
            }
        }

        // No active inputs remain, this is now complete
        self.active_node_indices.borrow_mut().remove(&tensor_idx);
        completed_node_indices.insert(tensor_idx);
        true
    }

    /// Iterate across all active tensors and move those that are now complete
    fn update_complete_tensors(&self) {
        for (idx, node) in self.graph.nodes().iter().enumerate() {
            if node.tensor().is_some() {
                self.update_complete_tensor(idx);
            }
        }
    }

    fn initialize_scheduler_state(&self) {
        let completed_node_indices = self.completed_node_indices.borrow();
        let mut unresolved_input_counts = vec![0; self.graph.nodes().len()];
        let mut ready_nodes_per_pe: HashMap<usize, BTreeSet<usize>> = HashMap::new();
        let mut remaining_nodes_per_pe = HashMap::new();

        for (pe_idx, node_indices) in &self.nodes_per_pe {
            let mut remaining_nodes = 0;
            for node_idx in node_indices {
                if completed_node_indices.contains(node_idx) {
                    continue;
                }

                remaining_nodes += 1;
                let unresolved_inputs = self.graph.nodes()[*node_idx]
                    .predecessors()
                    .iter()
                    .filter(|input_idx| !completed_node_indices.contains(input_idx))
                    .count();
                unresolved_input_counts[*node_idx] = unresolved_inputs;
                if unresolved_inputs == 0 {
                    ready_nodes_per_pe
                        .entry(*pe_idx)
                        .or_default()
                        .insert(*node_idx);
                }
            }
            remaining_nodes_per_pe.insert(*pe_idx, remaining_nodes);
        }

        *self.unresolved_input_counts.borrow_mut() = unresolved_input_counts;
        *self.ready_nodes_per_pe.borrow_mut() = ready_nodes_per_pe;
        *self.remaining_nodes_per_pe.borrow_mut() = remaining_nodes_per_pe;
    }

    fn mark_dependency_completed(&self, node_idx: usize) {
        let Some(pe_idx) = self.node_pe_indices[node_idx] else {
            return;
        };
        if self.completed_node_indices.borrow().contains(&node_idx)
            || self.active_node_indices.borrow().contains(&node_idx)
        {
            return;
        }

        let mut unresolved_input_counts = self.unresolved_input_counts.borrow_mut();
        let unresolved_inputs = &mut unresolved_input_counts[node_idx];
        if *unresolved_inputs == 0 {
            return;
        }

        *unresolved_inputs -= 1;
        if *unresolved_inputs == 0 {
            self.ready_nodes_per_pe
                .borrow_mut()
                .entry(pe_idx)
                .or_default()
                .insert(node_idx);
        }
    }

    fn mark_successors_updated(&self, node_idx: usize) {
        for output_node_idx in self.graph.nodes()[node_idx].successors() {
            self.mark_dependency_completed(*output_node_idx);
        }
    }

    pub fn total_tasks(&self) -> usize {
        self.graph.nodes().len()
    }

    #[must_use]
    pub fn num_graph_nodes_completed(&self) -> usize {
        self.completed_node_indices.borrow().len()
    }

    fn compute_views(&self, node_idx: usize) -> Result<ComputeTensorViews, SimError> {
        self.graph.compute_views(node_idx).ok_or_else(|| {
            SimError(format!(
                "node {} is not a compute node",
                self.graph.nodes()[node_idx].id()
            ))
        })
    }

    pub fn check_tasks_complete(&self) -> SimResult {
        let num_active = self.active_node_indices.borrow().len();
        if num_active != 0 {
            return sim_error!("{num_active} tasks still active");
        }

        let num_completed = self.completed_node_indices.borrow().len();
        let num_tasks = self.graph.nodes().len();
        if num_completed != num_tasks {
            return sim_error!(
                "{num_completed} tasks completed out of a total of {num_tasks} tasks."
            );
        }

        Ok(())
    }

    pub fn dump_stats(&self) -> SimResult {
        let mut total_load_bytes = 0usize;
        let mut total_store_bytes = 0usize;
        let mut machine_ops = MachineOpCounts::default();
        let mut num_compute_nodes = 0;
        let mut num_tensor_nodes = 0;
        for (idx, node) in self.graph.nodes().iter().enumerate() {
            match node.operation() {
                Some(op) => {
                    let views = self.compute_views(idx)?;
                    machine_ops = machine_ops
                        .checked_add(op.compute_machine_ops(views.inputs(), views.outputs())?)
                        .map_err(|error| SimError(format!("{}: {error}", node.id())))?;
                    for input_view in views.inputs().iter().flatten() {
                        add_to_byte_total(
                            &mut total_load_bytes,
                            input_view.layout().num_access_bytes(),
                            "load",
                        )?;
                    }
                    for output_view in views.outputs().iter().flatten() {
                        add_to_byte_total(
                            &mut total_store_bytes,
                            output_view.layout().num_access_bytes(),
                            "store",
                        )?;
                    }
                    num_compute_nodes += 1;
                }
                None => num_tensor_nodes += 1,
            }
        }

        info!(self.entity ; "Timetable:");
        info!(self.entity ;
            "  {num_compute_nodes} compute nodes, {num_tensor_nodes} tensor nodes"
        );
        info!(self.entity ; "  loads {total_load_bytes} bytes, stores {total_store_bytes} bytes");
        info!(self.entity ;
            "  machine ops {} total, {} add, {} mul, {} compare",
            machine_ops.checked_total()?,
            machine_ops.adds,
            machine_ops.muls,
            machine_ops.compares
        );

        Ok(())
    }

    /// Create map of node ID to status for rendering
    #[must_use]
    pub fn mermaid_node_statuses(&self) -> HashMap<String, MermaidNodeStatus> {
        let completed = self.completed_node_indices.borrow();
        let active = self.active_node_indices.borrow();

        self.graph
            .nodes()
            .iter()
            .enumerate()
            .map(|(idx, node)| {
                let status = if completed.contains(&idx) {
                    MermaidNodeStatus::Complete
                } else if active.contains(&idx) {
                    MermaidNodeStatus::Active
                } else {
                    MermaidNodeStatus::Pending
                };
                (node.id().to_string(), status)
            })
            .collect()
    }

    /// Render a mermaid of the current status of the Timetable
    #[must_use]
    pub fn render_mermaid(&self) -> String {
        render_mermaid(&self.graph, &self.mermaid_node_statuses())
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

fn add_to_byte_total(total: &mut usize, num_bytes: usize, kind: &str) -> SimResult {
    *total = total
        .checked_add(num_bytes)
        .ok_or_else(|| SimError(format!("Timetable {kind} byte total overflows")))?;
    Ok(())
}

#[async_trait(?Send)]
impl Dispatch for Timetable {
    fn task_by_id(&self, task_idx: usize) -> Result<Task, SimError> {
        let node = &self.graph.nodes()[task_idx];
        let Some(operation) = node.operation() else {
            return sim_error!("Task Index {task_idx} refers to a Tensor node");
        };
        let (inputs, outputs) = self.compute_views(task_idx)?.into_parts();
        Ok(build_compute_task(
            node.id(),
            operation.clone(),
            inputs,
            outputs,
        ))
    }

    fn set_task_active(&self, node_idx: usize) -> SimResult {
        debug!(self.entity; "task{node_idx}: active");
        if let Some(pe_idx) = self.node_pe_indices[node_idx] {
            self.ready_nodes_per_pe
                .borrow_mut()
                .entry(pe_idx)
                .or_default()
                .remove(&node_idx);
        }
        self.active_node_indices.borrow_mut().insert(node_idx);
        self.ready_nodes_changed.notify();
        Ok(())
    }

    fn set_task_completed(&self, node_idx: usize) -> SimResult {
        debug!(self.entity; "task{node_idx}: completed");

        if self.completed_node_indices.borrow().contains(&node_idx) {
            return Ok(());
        }

        let node = &self.graph.nodes()[node_idx];
        if let Some(pe_idx) = self.node_pe_indices[node_idx] {
            self.ready_nodes_per_pe
                .borrow_mut()
                .entry(pe_idx)
                .or_default()
                .remove(&node_idx);

            let mut remaining_nodes_per_pe = self.remaining_nodes_per_pe.borrow_mut();
            let remaining_nodes = remaining_nodes_per_pe.get_mut(&pe_idx).ok_or_else(|| {
                SimError(format!("No remaining node count for PE index {pe_idx}"))
            })?;
            if *remaining_nodes == 0 {
                return sim_error!("PE remaining node count underflow for task {node_idx}");
            }
            *remaining_nodes -= 1;
        }
        self.active_node_indices.borrow_mut().remove(&node_idx);
        self.completed_node_indices.borrow_mut().insert(node_idx);
        self.mark_successors_updated(node_idx);

        for tensor_node_idx in node.successors() {
            if self.graph.nodes()[*tensor_node_idx].tensor().is_some()
                && self.update_complete_tensor(*tensor_node_idx)
            {
                self.mark_successors_updated(*tensor_node_idx);
            }
        }

        self.ready_nodes_changed.notify();
        Ok(())
    }

    fn ready_task_indices(&self, pe_id: &str) -> Result<(bool, Vec<usize>), SimError> {
        trace!(self.entity ; "ready_node_indices for {pe_id}");
        let pe_idx = self.platform.pe_idx_from_name(pe_id)?;
        let pe_done = self
            .remaining_nodes_per_pe
            .borrow()
            .get(&pe_idx)
            .copied()
            .unwrap_or_default()
            == 0;
        let ready_node_indices = self
            .ready_nodes_per_pe
            .borrow()
            .get(&pe_idx)
            .map(|nodes| nodes.iter().copied().collect())
            .unwrap_or_default();

        debug!(self.entity; "PE {pe_id}: done: {pe_done}, ready indices: {ready_node_indices:?}");
        Ok((pe_done, ready_node_indices))
    }

    async fn wait_for_change(&self) {
        self.ready_nodes_changed.listen().await;
    }

    fn total_tasks_for_pe(&self, pe_name: &str) -> usize {
        let Ok(pe_idx) = self.platform.pe_idx_from_name(pe_name) else {
            return 0;
        };
        self.nodes_per_pe
            .get(&pe_idx)
            .map(BTreeSet::len)
            .unwrap_or_default()
    }
}
