// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
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
use gwr_models::processing_element::task::{ProgramTaskConfig, Task};
use gwr_platform::Platform;
use gwr_track::entity::Entity;
use gwr_track::{debug, info, trace};

pub mod atomic;
pub mod graph;
pub mod mermaid;
mod program_compose;
pub mod scheduled;
pub mod timetable_file;
pub use graph::{
    ComputeTensorDirection, ComputeTensorViews, EdgeEndpoint, TensorConnection, TimetableEdge,
    TimetableGraph, TimetableNode,
};

use crate::mermaid::{MermaidNodeStatus, render_mermaid};

#[derive(EntityGet)]
/// A validated workload graph that can dispatch tasks onto a [`Platform`].
///
/// `Timetable` owns workload structure, task dependencies, tensor views, and PE
/// placement. It accepts a graph whose file sections have already been
/// validated and checks PE placement before tasks are dispatched, so topology
/// and reference errors are reported before the simulation relies on them.
pub struct Timetable {
    entity: Rc<Entity>,
    platform: Rc<Platform>,
    graph: TimetableGraph,
    atomic_tasks: HashMap<usize, gwr_models::processing_element::task::AtomicTaskConfig>,
    atomic_joins: HashMap<usize, gwr_models::processing_element::task::AtomicJoinConfig>,
    streamed_tasks: HashMap<usize, Rc<gwr_models::processing_element::task::StreamedOutputSlot>>,
    node_pe_indices: Vec<Option<usize>>,
    completed_node_indices: RefCell<HashSet<usize>>,
    completed_workload: Cell<Option<Workload>>,
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

        let mut atomic_tasks = HashMap::new();
        let mut atomic_joins = HashMap::new();
        let mut streamed_tasks = HashMap::new();
        let node_indices = graph
            .nodes()
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id(), i))
            .collect::<HashMap<_, _>>();
        if let Some(admitted) = graph.atomic_effects() {
            for region in admitted.memories() {
                let target = platform
                    .atomic_memory_by_device(region.device)?
                    .atomic_target()
                    .unwrap();
                target.validate_region(
                    region.device,
                    region.base,
                    region.bytes,
                    region.path,
                    &region.service,
                )?;
            }
            // Resolve every actual physical issuer before registering any
            // dynamic metadata. No live request or source ID is minted.
            for claim in admitted.claims() {
                let index = *node_indices.get(claim.node.as_str()).unwrap();
                let pe = graph.nodes()[index]
                    .pe()
                    .ok_or_else(|| SimError("atomic role lacks assigned PE".into()))?;
                platform.validate_atomic_issuer(pe, claim.device, claim.address)?;
            }
            // Resolve every source slot against actual PE/map/paired ports
            // before registering groups. No owners or source IDs are minted.
            let mut output_slots = HashMap::new();
            for stream in admitted.streams() {
                let index = *node_indices.get(stream.node.as_str()).unwrap();
                let pe = platform.pe(graph.nodes()[index].pe().unwrap())?;
                let links = admitted
                    .groups()
                    .iter()
                    .find(|g| {
                        (g.group.invocation, g.group.generation)
                            == (stream.config.invocation, stream.config.generation)
                    })
                    .unwrap();
                let target = platform
                    .atomic_memory_by_device(links.group.device)?
                    .atomic_target()
                    .unwrap();
                let slot = gwr_models::processing_element::task::StreamedOutputSlot::new_bound(
                    stream.config.clone(),
                    pe,
                    target,
                )?;
                for (id, offset) in &stream.outputs {
                    if output_slots
                        .insert(id.clone(), (slot.clone(), *offset))
                        .is_some()
                    {
                        return Err(SimError("duplicate source scratch output".into()));
                    }
                }
                streamed_tasks.insert(index, slot);
            }
            let mut controllers = HashMap::new();
            for links in admitted.groups() {
                let memory = platform.atomic_memory_by_device(links.group.device)?;
                let index = *node_indices.get(links.final_join.as_str()).unwrap();
                let pe = graph.nodes()[index]
                    .pe()
                    .ok_or_else(|| SimError("atomic final join requires actual PE task".into()))?;
                platform.validate_atomic_issuer(
                    pe,
                    links.group.device,
                    links.group.output_words[0],
                )?;
                let bound = gwr_models::memory::atomic::SplitKController::new(
                    links.group.clone(),
                    memory.atomic_target().unwrap(),
                    memory.atomic_control().unwrap(),
                )?;
                let word = gwr_models::memory::atomic::AtomicWord {
                    device: links.group.device,
                    address: links.group.output_words[0],
                    invocation: links.group.invocation,
                    generation: links.group.generation,
                    operation: gwr_models::memory::atomic::AtomicOperation::Bf16PairFadd,
                    scope: gwr_models::memory::atomic::AtomicScope::Agent,
                    ordering: gwr_models::memory::atomic::AtomicOrdering::Monotonic,
                };
                let binding = platform.bind_atomic_controlled_task(
                    pe,
                    links.final_join.clone(),
                    links.group.device,
                    word.address,
                    gwr_models::memory::atomic::Effect::Atomic {
                        participant: links.group.publisher,
                        word,
                    },
                    bound.clone(),
                )?;
                atomic_joins.insert(
                    index,
                    gwr_models::processing_element::task::AtomicJoinConfig::new(
                        binding,
                        bound.clone(),
                    )?,
                );
                controllers.insert((links.group.invocation, links.group.generation), bound);
            }
            for claim in admitted.claims() {
                let index = *node_indices
                    .get(claim.node.as_str())
                    .ok_or_else(|| SimError("admitted atomic node disappeared".into()))?;
                let pe_name = graph.nodes()[index]
                    .pe()
                    .ok_or_else(|| SimError("atomic task lacks assigned PE".into()))?;
                let identity = match claim.effect {
                    gwr_models::memory::atomic::Effect::Atomic { word, .. } => {
                        Some((word.invocation, word.generation))
                    }
                    gwr_models::memory::atomic::Effect::Control {
                        invocation,
                        generation,
                        ..
                    }
                    | gwr_models::memory::atomic::Effect::InitializeOutput {
                        invocation,
                        generation,
                    } => Some((invocation, generation)),
                    _ => None,
                };
                let selected = identity.and_then(|key| controllers.get(&key));
                let config = if let Some(controller) = selected {
                    platform.bind_atomic_controlled_task(
                        pe_name,
                        claim.node.clone(),
                        claim.device,
                        claim.address,
                        claim.effect,
                        controller.clone(),
                    )?
                } else {
                    platform.bind_atomic_task(
                        pe_name,
                        claim.node.clone(),
                        claim.device,
                        claim.address,
                        claim.effect,
                    )?
                };
                let config = if let Some((slot, offset)) = output_slots.get(&claim.node) {
                    config.with_streamed_operand(slot.clone(), *offset)?
                } else {
                    config
                };
                atomic_tasks.insert(index, config);
            }
        }
        let timetable = Self {
            atomic_tasks,
            atomic_joins,
            streamed_tasks,
            entity,
            graph,
            node_pe_indices,
            platform: platform.clone(),
            completed_node_indices: RefCell::new(HashSet::new()),
            completed_workload: Cell::new(None),
            active_node_indices: RefCell::new(HashSet::new()),
            nodes_per_pe,
            ready_nodes_per_pe: RefCell::new(HashMap::new()),
            remaining_nodes_per_pe: RefCell::new(HashMap::new()),
            unresolved_input_counts: RefCell::new(Vec::new()),
            ready_nodes_changed: Repeated::new(()),
        };

        for (idx, node) in timetable.graph.nodes().iter().enumerate() {
            if node.is_task() {
                let _ = timetable.task_by_id(idx)?;
            }
        }

        timetable.update_complete_tensors();
        timetable.initialize_scheduler_state();

        Ok(timetable)
    }

    /// Validate typed capabilities, per-tile SRAM, and transfer memory mapping
    /// without starting the simulation or issuing memory requests.
    pub fn preflight(&self) -> SimResult {
        for (index, node) in self.graph.nodes().iter().enumerate() {
            if !node.is_task() {
                continue;
            }
            let pe_name = node
                .pe()
                .ok_or_else(|| SimError(format!("Task '{}' has no PE placement", node.id())))?;
            let pe = self.platform.pe(pe_name)?;
            match self.task_by_id(index)? {
                Task::StreamedMatrix { config } => config.validate_for_pe(pe),
                Task::ProgramTask { config } => pe.preflight_program_task(&config),
                Task::TransferTask { config } => pe.preflight_transfer_task(&config),
                Task::AtomicEffect { config } => pe.preflight_atomic_task(&config),
                Task::AtomicControlJoin { config } => config.validate_for_pe(pe),
                Task::SyncTask { .. } => Ok(()),
            }
            .map_err(|error| {
                SimError(format!("Node '{}' on PE '{pe_name}': {error}", node.id()))
            })?;
        }
        Ok(())
    }

    /// Iterate across all active tensors and move those that are now complete
    fn update_complete_tensors(&self) {
        for idx in self.graph.topological_order() {
            if self.graph.nodes()[*idx].tensor().is_some() {
                self.update_complete_tensor(*idx);
            }
        }
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
        if let Some(mut completed_workload) = self.completed_workload.get() {
            completed_workload.nodes += 1;
            self.completed_workload.set(Some(completed_workload));
        }
        true
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

    fn mark_successors_updated(&self, node_idx: usize) {
        let mut completed = VecDeque::from([node_idx]);
        while let Some(node_idx) = completed.pop_front() {
            for successor in self.graph.nodes()[node_idx].successors() {
                if self.graph.nodes()[*successor].tensor().is_some() {
                    if self.update_complete_tensor(*successor) {
                        completed.push_back(*successor);
                    }
                } else {
                    self.mark_dependency_completed(*successor);
                }
            }
        }
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

    pub fn total_tasks(&self) -> usize {
        self.graph.nodes().len()
    }

    /// Calculate the planned boundary traffic and node count for this
    /// timetable.
    pub fn workload_totals(&self) -> Result<Workload, SimError> {
        self.calculate_workload(0..self.graph.nodes().len())
    }

    /// Start tracking completed workload, including nodes already complete.
    pub fn enable_completed_workload_tracking(&self) -> SimResult {
        let workload =
            self.calculate_workload(self.completed_node_indices.borrow().iter().copied())?;
        self.completed_workload.set(Some(workload));
        Ok(())
    }

    /// Return completed workload when tracking is enabled.
    #[must_use]
    pub fn completed_workload(&self) -> Option<Workload> {
        self.completed_workload.get()
    }

    /// Return the number of graph nodes currently executing.
    #[must_use]
    pub fn num_graph_nodes_active(&self) -> usize {
        self.active_node_indices.borrow().len()
    }

    fn calculate_workload(
        &self,
        node_indices: impl IntoIterator<Item = usize>,
    ) -> Result<Workload, SimError> {
        let mut workload = Workload::default();
        for node_idx in node_indices {
            self.add_node_workload(node_idx, &mut workload)?;
        }
        Ok(workload)
    }

    fn add_node_workload(&self, node_idx: usize, workload: &mut Workload) -> SimResult {
        workload.nodes += 1;
        if self.graph.nodes()[node_idx].is_task() {
            let (read, written) = match self.task_by_id(node_idx)? {
                Task::StreamedMatrix { config } => (config.config().chunk_count() * 24576, 0),
                Task::ProgramTask { config } => config.planned_traffic_bytes()?,
                Task::TransferTask { config } => config.planned_traffic_bytes()?,
                Task::AtomicEffect { .. }
                | Task::AtomicControlJoin { .. }
                | Task::SyncTask { .. } => (0, 0),
            };
            add_to_byte_total(&mut workload.read_bytes, read, "planned load")?;
            add_to_byte_total(&mut workload.written_bytes, written, "planned store")?;
        }
        Ok(())
    }

    #[must_use]
    pub fn num_graph_nodes_completed(&self) -> usize {
        self.completed_node_indices.borrow().len()
    }

    /// Machine operations completed by the PEs used by this timetable.
    pub fn machine_operations_executed(&self) -> Result<usize, SimError> {
        let pe_names: BTreeSet<_> = self
            .graph
            .nodes()
            .iter()
            .filter_map(|node| node.pe())
            .collect();
        pe_names.into_iter().try_fold(0usize, |total, name| {
            let ops = self
                .platform
                .pe(name)?
                .machine_ops()
                .checked_operation_total()?;
            total
                .checked_add(ops)
                .ok_or_else(|| SimError("Timetable machine operation total overflows".into()))
        })
    }

    fn compute_views(&self, node_idx: usize) -> Result<ComputeTensorViews, SimError> {
        self.graph.compute_views(node_idx).ok_or_else(|| {
            SimError(format!(
                "node {} is not a compute node",
                self.graph.nodes()[node_idx].id()
            ))
        })
    }

    pub fn atomic_group_specs(&self) -> Vec<gwr_models::memory::atomic::ControlGroup> {
        self.graph
            .atomic_effects()
            .map(|a| a.groups().iter().map(|g| g.group.clone()).collect())
            .unwrap_or_default()
    }
    pub fn atomic_control_observations(
        &self,
    ) -> Vec<gwr_models::memory::atomic::ControllerObservation> {
        self.atomic_control_observations_by_join()
            .into_values()
            .collect()
    }
    pub fn atomic_control_observations_by_join(
        &self,
    ) -> std::collections::BTreeMap<String, gwr_models::memory::atomic::ControllerObservation> {
        self.atomic_joins
            .iter()
            .map(|(index, config)| {
                (
                    self.graph.nodes()[*index].id().to_string(),
                    config.observation(),
                )
            })
            .collect()
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
        let mut planned_load_bytes = 0usize;
        let mut planned_store_bytes = 0usize;
        let mut machine_ops = MachineOpCounts::default();
        let mut peak_pe_sram_bytes = 0usize;
        let mut pe_names = BTreeSet::new();
        let mut num_compute_nodes = 0;
        let mut num_transfer_nodes = 0;
        let mut num_tensor_nodes = 0;
        for (index, node) in self.graph.nodes().iter().enumerate() {
            if node.is_task() {
                {
                    let (read, written) = match self.task_by_id(index)? {
                        Task::StreamedMatrix { config } => {
                            (config.config().chunk_count() * 24576, 0)
                        }
                        Task::ProgramTask { config } => config.planned_traffic_bytes()?,
                        Task::TransferTask { config } => config.planned_traffic_bytes()?,
                        Task::AtomicEffect { .. }
                        | Task::AtomicControlJoin { .. }
                        | Task::SyncTask { .. } => (0, 0),
                    };
                    add_to_byte_total(&mut planned_load_bytes, read, "planned load")?;
                    add_to_byte_total(&mut planned_store_bytes, written, "planned store")?;
                }
                if let Some(pe) = node.pe() {
                    pe_names.insert(pe);
                }
                if node.is_transfer() {
                    num_transfer_nodes += 1;
                } else {
                    num_compute_nodes += 1;
                }
            } else {
                num_tensor_nodes += 1;
            }
        }
        for pe in pe_names {
            let element = self.platform.pe(pe)?;
            machine_ops = machine_ops.checked_add(element.machine_ops())?;
            let (read, written) = element.memory_traffic_bytes();
            add_to_byte_total(&mut total_load_bytes, read, "load")?;
            add_to_byte_total(&mut total_store_bytes, written, "store")?;
            peak_pe_sram_bytes = peak_pe_sram_bytes.max(element.peak_sram_bytes());
        }

        info!(self.entity ; "Timetable:");
        info!(self.entity ;
            "  {num_compute_nodes} compute nodes, {num_transfer_nodes} transfer nodes, {num_tensor_nodes} tensor nodes"
        );
        info!(self.entity ; "  planned loads {planned_load_bytes} bytes, stores {planned_store_bytes} bytes");
        info!(self.entity ; "  actual loads {total_load_bytes} bytes, stores {total_store_bytes} bytes");
        info!(self.entity ; "  peak PE SRAM {peak_pe_sram_bytes} bytes");
        info!(self.entity ;
            "  machine ops {:?}, {} FLOP-equivalents",
            machine_ops.by_operation,
            machine_ops.flop_equivalents(),
        );

        Ok(())
    }

    /// Render a mermaid of the current status of the Timetable
    #[must_use]
    pub fn render_mermaid(&self) -> String {
        render_mermaid(&self.graph, &self.mermaid_node_statuses())
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
}

#[async_trait(?Send)]
impl Dispatch for Timetable {
    fn task_by_id(&self, task_idx: usize) -> Result<Task, SimError> {
        let node = &self.graph.nodes()[task_idx];
        if !node.is_task() {
            return sim_error!("Task Index {task_idx} refers to a Tensor node");
        }
        if let Some(config) = self.streamed_tasks.get(&task_idx) {
            return Ok(Task::StreamedMatrix {
                config: config.clone(),
            });
        }
        if let Some(config) = self.atomic_joins.get(&task_idx) {
            return Ok(Task::AtomicControlJoin {
                config: Box::new(config.clone()),
            });
        }
        if let Some(config) = self.atomic_tasks.get(&task_idx) {
            config.validate()?;
            return Ok(Task::AtomicEffect {
                config: Box::new(config.clone()),
            });
        }
        let (inputs, outputs) = self.compute_views(task_idx)?.into_parts();
        if node.is_transfer() {
            let config = gwr_models::transfer::TransferTaskConfig {
                id: node.id().into(),
                source: inputs
                    .first()
                    .and_then(Clone::clone)
                    .ok_or_else(|| SimError("unbound transfer source".into()))?,
                destination: outputs
                    .first()
                    .and_then(Clone::clone)
                    .ok_or_else(|| SimError("unbound transfer destination".into()))?,
                buffer_bytes: node.transfer_buffer_bytes().unwrap(),
            };
            config.validate()?;
            return Ok(Task::TransferTask {
                config: Box::new(config),
            });
        }
        if let Some(program) = node.program() {
            return Ok(Task::ProgramTask {
                config: Box::new(ProgramTaskConfig {
                    id: node.id().to_owned(),
                    program: program.clone(),
                    tiling: node.tiling().cloned(),
                    pipeline: node.pipeline().cloned(),
                    inputs: inputs
                        .into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            SimError(format!("program {} has unbound input", node.id()))
                        })?,
                    outputs: outputs
                        .into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            SimError(format!("program {} has unbound output", node.id()))
                        })?,
                }),
            });
        }
        sim_error!("compute node '{}' has no program", node.id())
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

        let completed_workload = if let Some(mut workload) = self.completed_workload.get() {
            self.add_node_workload(node_idx, &mut workload)?;
            Some(workload)
        } else {
            None
        };

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
        if let Some(completed_workload) = completed_workload {
            self.completed_workload.set(Some(completed_workload));
        }
        self.mark_successors_updated(node_idx);

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

/// Graph nodes and the boundary traffic planned for their program tasks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Workload {
    /// Number of graph nodes in the timetable.
    pub nodes: usize,
    /// Bytes planned for program input reads.
    pub read_bytes: usize,
    /// Bytes planned for program output writes.
    pub written_bytes: usize,
}

fn add_to_byte_total(total: &mut usize, num_bytes: usize, kind: &str) -> SimResult {
    *total = total
        .checked_add(num_bytes)
        .ok_or_else(|| SimError(format!("Timetable {kind} byte total overflows")))?;
    Ok(())
}
