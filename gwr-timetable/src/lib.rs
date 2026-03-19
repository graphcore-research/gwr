// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::sim_error;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::EntityGet;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::task::{
    ComputeOp, ComputeTaskConfig, MemoryOp, MemoryTaskConfig, Task,
};
use gwr_platform::Platform;
use gwr_track::entity::Entity;
use gwr_track::{debug, trace};

pub mod timetable_file;
pub mod types;
use timetable_file::{NodeSection, TimetableFile};
use types::Node;

use crate::timetable_file::{MemoryConfigSection, TensorConfigSection, dtype_num_bytes};

fn validate_access_in_range(
    mem_node: &Node,
    mem_config: &MemoryConfigSection,
    tensor_config: &TensorConfigSection,
) -> SimResult {
    let num_bytes = dtype_num_bytes(&tensor_config.dtype, mem_config.num_elements);
    let load_end = mem_config.offset + num_bytes;
    if load_end > tensor_config.num_bytes() {
        sim_error!(
            "Memory node '{}' accesses memory outside Tensor node",
            mem_node.node_section.id()
        )
    } else {
        Ok(())
    }
}

#[derive(EntityGet)]
pub struct Timetable {
    entity: Rc<Entity>,
    platform: Rc<Platform>,
    nodes: Vec<Node>,
    completed_node_indices: RefCell<HashSet<usize>>,
    active_node_indices: RefCell<HashSet<usize>>,
    nodes_per_pe: RefCell<HashMap<usize, HashSet<usize>>>,
    active_tensor_node_indices: RefCell<HashSet<usize>>,
    ready_nodes_changed: Repeated<()>,
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
        let mut active_tensor_node_indices = HashSet::new();
        for (i, node_section) in timetable_file.nodes.drain(..).enumerate() {
            let (id, pe) = node_section.id_pe();
            node_idx_by_id.insert(id.clone(), i);

            if let Some(pe_id) = &pe {
                let pe_idx = platform.pe_idx_from_name(pe_id)?;
                nodes_per_pe
                    .entry(pe_idx)
                    .or_insert_with(HashSet::new)
                    .insert(i);
            }

            if let NodeSection::Tensor { id: _, config: _ } = node_section {
                active_tensor_node_indices.insert(i);
            }

            nodes.push(Node {
                node_section,
                from_nodes: HashSet::new(),
                to_nodes: HashSet::new(),
            });
        }

        for edge_section in &timetable_file.edges {
            // Note: we have validated the edges so we can just unwrap()
            let from_node_idx = *node_idx_by_id.get(&edge_section.from).unwrap();
            let to_node_idx = *node_idx_by_id.get(&edge_section.to).unwrap();

            nodes[from_node_idx].to_nodes.insert(to_node_idx);
            nodes[to_node_idx].from_nodes.insert(from_node_idx);
        }

        let timetable = Self {
            entity,
            nodes,
            platform: platform.clone(),
            completed_node_indices: RefCell::new(HashSet::new()),
            active_node_indices: RefCell::new(HashSet::new()),
            nodes_per_pe: RefCell::new(nodes_per_pe),
            ready_nodes_changed: Repeated::new(()),
            active_tensor_node_indices: RefCell::new(active_tensor_node_indices),
        };

        timetable.validate()?;

        timetable.update_complete_tensors();

        Ok(timetable)
    }

    fn validate(&self) -> SimResult {
        // Ensure that all Load nodes connect from a Tensor node and that all
        // Store nodes connect to a Tensor node
        for node in &self.nodes {
            match &node.node_section {
                NodeSection::Memory {
                    id: _,
                    op,
                    pe: _,
                    config,
                } => match op {
                    MemoryOp::Load => {
                        self.validate_load_node(node, config)?;
                    }
                    MemoryOp::Store => {
                        self.validate_store_node(node, config)?;
                    }
                },
                _ => {
                    // Do nothing
                }
            }
        }

        Ok(())
    }

    fn get_tensor_config_for_idx(&self, node_idx: usize) -> Option<&TensorConfigSection> {
        let node = &self.nodes[node_idx];
        self.get_tensor_node_config(node)
    }

    /// Given a Node, return the input Tensor config for a Memory Load and the
    /// output Tensor config for a Memory Store. In all other cases returns
    /// None.
    fn get_tensor_node_config(&self, node: &Node) -> Option<&TensorConfigSection> {
        node.get_tensor_node_idx().map(|node_idx| {
            let node = &self.nodes[node_idx];
            if let NodeSection::Tensor { id: _, config } = &node.node_section {
                Some(config)
            } else {
                None
            }
        })?
    }

    fn validate_load_node(&self, load_node: &Node, load_config: &MemoryConfigSection) -> SimResult {
        if load_node.from_nodes.len() != 1 {
            return sim_error!(
                "{} edges connect into Load node '{}'",
                load_node.from_nodes.len(),
                load_node.node_section.id()
            );
        }

        match self.get_tensor_node_config(load_node) {
            Some(config) => validate_access_in_range(load_node, load_config, config),
            None => sim_error!(
                "Load node '{}' not connected from Tensor node",
                load_node.node_section.id()
            ),
        }
    }

    fn validate_store_node(
        &self,
        store_node: &Node,
        store_config: &MemoryConfigSection,
    ) -> SimResult {
        if store_node.to_nodes.len() != 1 {
            return sim_error!(
                "{} edges connect from Store node '{}'",
                store_node.to_nodes.len(),
                store_node.node_section.id()
            );
        }

        match self.get_tensor_node_config(store_node) {
            Some(config) => validate_access_in_range(store_node, store_config, config),
            None => sim_error!(
                "Store node '{}' not connected to Tensor node",
                store_node.node_section.id()
            ),
        }
    }

    /// Check a given tensor index and move it if it is now complete
    fn update_complete_tensor(&self, tensor_idx: usize) {
        let mut completed_node_indices = self.completed_node_indices.borrow_mut();
        let tensor_node = &self.nodes[tensor_idx];

        // Look for an input node that is not complete
        let active_input_index = tensor_node
            .from_nodes
            .iter()
            .find(|input_node_idx| !completed_node_indices.contains(input_node_idx));

        if active_input_index.is_none() {
            // No active inputs remain, this is now complete
            self.active_node_indices.borrow_mut().remove(&tensor_idx);
            completed_node_indices.insert(tensor_idx);
        }
    }

    /// Iterate across all active tensors and move those that are now complete
    fn update_complete_tensors(&self) {
        // Need to take a clone of the tensor indices as they will be updated as we walk
        // through them
        let tensor_indices = self.active_tensor_node_indices.borrow().clone();
        for tensor_idx in tensor_indices {
            self.update_complete_tensor(tensor_idx);
        }
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
        let address = tensor_config.addr + config.offset as u64;
        let bits_per_element = tensor_config.bits_per_element();
        let num_bytes = (config.num_elements * bits_per_element).div_ceil(8);
        (address, num_bytes)
    }

    /// Given a node index return the number of bytes that node will receive.
    ///
    /// If the node defines its own number of bytes then return that. Otherwise,
    /// follow each of the input edges to determine how many bytes it will
    /// consume.
    ///
    /// An error will be returned if it is unable to determine the number of
    /// bytes or if different inputs define inconsistent number of bytes.
    pub fn bytes_for_node(&self, node_idx: usize) -> Result<usize, SimError> {
        let node = &self.nodes[node_idx];
        if let NodeSection::Memory {
            id: _,
            op: _,
            pe: _,
            config,
        } = &node.node_section
        {
            // Note we assume that the graph has been validated so that we can simply unwrap
            // the result
            let tensor_config = self.get_tensor_config_for_idx(node_idx).unwrap();
            let bits_per_element = tensor_config.bits_per_element();
            let num_bytes = (config.num_elements * bits_per_element).div_ceil(8);
            return Ok(num_bytes);
        }

        let (id, _) = node.node_section.id_pe();
        let mut num_bytes_values = HashSet::new();
        // Iterate through all the edges that provide inputs to the node and record how
        // many bytes they say there should be.
        for from_node_idx in &node.from_nodes {
            let node_num_bytes = self.bytes_for_node(*from_node_idx)?;
            num_bytes_values.insert(node_num_bytes);
        }

        match num_bytes_values.len() {
            0 => sim_error!("Unable to determine num bytes for node {}", id),
            1 => Ok(num_bytes_values.into_iter().next().unwrap()),
            _ => sim_error!(
                "Inconsistent input num_bytes for {} ({:?})",
                id,
                num_bytes_values
            ),
        }
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

    pub fn dump_stats(&self) {
        let mut total_load_bytes = 0;
        let mut total_store_bytes = 0;
        for node in &self.nodes {
            match &node.node_section {
                NodeSection::Memory {
                    id: _,
                    op,
                    pe: _,
                    config,
                } => {
                    let (_, num_bytes) = self.memory_access_address_num_bytes(node, config);
                    match op {
                        MemoryOp::Load => total_load_bytes += num_bytes,
                        MemoryOp::Store => total_store_bytes += num_bytes,
                    }
                }
                _ => {
                    // Ignore
                }
            }
        }
        println!(
            "Timetable contains {total_load_bytes} load bytes, {total_store_bytes} store bytes"
        );
    }
}

fn build_compute_task(op: ComputeOp, num_bytes: usize) -> Task {
    Task::ComputeTask {
        config: ComputeTaskConfig { op, num_bytes },
    }
}

fn build_memory_task(op: MemoryOp, addr: u64, num_bytes: usize) -> Task {
    Task::MemoryTask {
        config: MemoryTaskConfig {
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
            NodeSection::Compute { id: _, op, pe: _ } => {
                let num_bytes = self.bytes_for_node(task_idx)?;
                Ok(build_compute_task(*op, num_bytes))
            }
            NodeSection::Memory {
                id: _,
                op,
                pe: _,
                config,
            } => {
                let (address, num_bytes) = self.memory_access_address_num_bytes(node, config);
                Ok(build_memory_task(*op, address, num_bytes))
            }
            NodeSection::Tensor { id: _, config: _ } => {
                sim_error!("Task Index {task_idx} refers to a Tensor node")
            }
        }
    }

    fn set_task_active(&self, node_idx: usize) -> SimResult {
        debug!(self.entity; "task{node_idx}: active");
        self.active_node_indices.borrow_mut().insert(node_idx);
        self.ready_nodes_changed.notify()?;
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

        if node.is_store_node() {
            let tensor_node_idx = node.get_tensor_node_idx().unwrap();
            self.update_complete_tensor(tensor_node_idx);
        }

        self.ready_nodes_changed.notify()?;
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
                for from_node_idx in &to_node.from_nodes {
                    if let Some(to_pe_id) = to_pe {
                        trace!(self.entity ; "-> {to_pe_id}");
                        let to_pe_idx = self.platform.pe_idx_from_name(to_pe_id)?;
                        trace!(self.entity ; "idx {to_pe_idx}");
                        if to_pe_idx == pe_idx && !completed_guard.contains(from_node_idx) {
                            trace!(self.entity ; "{node_idx} not ready");
                            ready = false;
                            break;
                        }
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
