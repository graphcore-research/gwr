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

pub mod types;
use types::{EdgeSection, Graph, NodeSection};

#[derive(EntityGet)]
pub struct Timetable {
    entity: Rc<Entity>,
    graph: Graph,
    platform: Rc<Platform>,
    node_idx_by_id: HashMap<String, usize>,
    completed_node_indices: RefCell<HashSet<usize>>,
    active_node_indices: RefCell<HashSet<usize>>,
    nodes_per_pe: RefCell<HashMap<usize, HashSet<usize>>>,
    edges_to_node: RefCell<HashMap<usize, HashSet<usize>>>,
    ready_nodes_changed: Repeated<()>,
}

impl Timetable {
    /// Create a Timetable from a Graph (build helper structures)
    pub fn new(
        parent: &Rc<Entity>,
        graph: Graph,
        platform: &Rc<Platform>,
    ) -> Result<Self, SimError> {
        graph.validate(platform)?;

        let entity = Rc::new(Entity::new(parent, "timetable"));
        let mut node_idx_by_id = HashMap::new();
        let mut nodes_per_pe = HashMap::new();
        for (i, node) in graph.nodes.iter().enumerate() {
            let (id, pe) = node.id_pe();
            node_idx_by_id.insert(id.clone(), i);

            if let Some(pe_id) = &pe {
                let pe_idx = platform.pe_idx_from_name(pe_id)?;
                nodes_per_pe
                    .entry(pe_idx)
                    .or_insert_with(HashSet::new)
                    .insert(i);
            }
        }

        let mut edges_to_node = HashMap::new();
        for (i, edge) in graph.edges.iter().enumerate() {
            let to_node_idx = node_idx_by_id.get(&edge.to).unwrap();
            edges_to_node
                .entry(*to_node_idx)
                .or_insert_with(HashSet::new)
                .insert(i);
        }

        Ok(Self {
            entity,
            graph,
            platform: platform.clone(),
            node_idx_by_id,
            completed_node_indices: RefCell::new(HashSet::new()),
            active_node_indices: RefCell::new(HashSet::new()),
            nodes_per_pe: RefCell::new(nodes_per_pe),
            edges_to_node: RefCell::new(edges_to_node),
            ready_nodes_changed: Repeated::new(()),
        })
    }

    pub fn node_for(&self, node_idx: usize) -> &NodeSection {
        &self.graph.nodes[node_idx]
    }

    pub fn edge_for(&self, edge_idx: usize) -> &EdgeSection {
        &self.graph.edges[edge_idx]
    }

    #[must_use]
    pub fn num_graph_nodes_completed(&self) -> usize {
        self.completed_node_indices.borrow().len()
    }

    pub fn check_tasks_complete(&self) -> SimResult {
        let num_active = self.active_node_indices.borrow().len();
        if num_active != 0 {
            return sim_error!("{num_active} tasks still active");
        }

        let num_completed = self.completed_node_indices.borrow().len();
        let num_tasks = self.node_idx_by_id.len();
        if num_completed != num_tasks {
            return sim_error!(
                "{num_completed} tasks completed out of a total of {num_tasks} tasks."
            );
        }

        Ok(())
    }
}

fn build_compute_task(op: ComputeOp) -> Task {
    Task::ComputeTask {
        config: ComputeTaskConfig { op },
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
        let node = &self.graph.nodes[task_idx];
        match &node {
            NodeSection::Compute {
                id: _,
                op,
                pe: _,
                config: _,
            } => Ok(build_compute_task(*op)),
            NodeSection::Memory {
                id: _,
                op,
                pe: _,
                config,
            } => {
                let num_bytes = config.num_bytes as usize;
                Ok(build_memory_task(*op, config.addr, num_bytes))
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

        let node = self.node_for(node_idx);
        let pe = node.pe();
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

                let to_node = self.node_for(*node_idx);
                let to_pe = to_node.pe();
                if let Some(edge_indices) = self.edges_to_node.borrow().get(node_idx) {
                    for edge_idx in edge_indices {
                        let edge = &self.graph.edges[*edge_idx];
                        if let Some(to_pe_id) = to_pe {
                            trace!(self.entity ; "-> {to_pe_id}");
                            let to_pe_idx = self.platform.pe_idx_from_name(to_pe_id)?;
                            trace!(self.entity ; "idx {to_pe_idx}");
                            let from_node_idx = self.node_idx_by_id[&edge.from];
                            if to_pe_idx == pe_idx && !completed_guard.contains(&from_node_idx) {
                                trace!(self.entity ; "{node_idx} not ready");
                                ready = false;
                                break;
                            }
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
        for node in &self.graph.nodes {
            let pe = node.pe();
            if let Some(pe) = pe
                && pe == pe_name
            {
                num_nodes += 1;
            }
        }
        num_nodes
    }
}
