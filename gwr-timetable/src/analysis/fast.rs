// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Aggregate activity-level roofline estimate. This deliberately avoids
//! producing per-event service windows or rescanning active nodes.

use std::collections::{BTreeMap, HashMap, VecDeque};

use gwr_engine::types::SimError;

/// Approximate duration and normalized service demand for one activity.
///
/// `resources` maps each shared resource to the ticks it would require at full
/// capacity. Predecessors refer to indices in the same slice.
#[derive(Clone, Debug)]
pub struct ActivityEstimate {
    pub pe: String,
    pub local_ticks: f64,
    /// Accumulated work on each independently serviceable compute engine.
    /// Keys should include the PE identity.
    pub engine_demands: BTreeMap<String, f64>,
    /// Compute activity indices in the input slice.
    pub predecessors: Vec<usize>,
    pub resources: BTreeMap<String, f64>,
}

/// Return an aggregate makespan floor based on per-engine work, dependency
/// depth, and shared-resource demand. Runs in O(N + E + I + R), where I is the
/// number of resource-demand entries and R the number of distinct resources.
pub fn estimate_makespan(activities: &[ActivityEstimate]) -> Result<f64, SimError> {
    let count = activities.len();
    let mut successors = vec![Vec::new(); count];
    let mut indegree = vec![0usize; count];
    let mut engine_work = HashMap::<&str, f64>::new();
    let mut resource_work = HashMap::<&str, f64>::new();

    for (index, activity) in activities.iter().enumerate() {
        if !activity.local_ticks.is_finite() || activity.local_ticks < 0.0 {
            return Err(SimError(format!(
                "Invalid local duration for activity {index}"
            )));
        }
        for (engine, demand) in &activity.engine_demands {
            if !demand.is_finite() || *demand < 0.0 {
                return Err(SimError(format!(
                    "Invalid compute demand for engine '{engine}' in activity {index}"
                )));
            }
            *engine_work.entry(engine).or_default() += demand;
        }
        for (resource, demand) in &activity.resources {
            if !demand.is_finite() || *demand < 0.0 {
                return Err(SimError(format!(
                    "Invalid demand for resource '{resource}' in activity {index}"
                )));
            }
            *resource_work.entry(resource).or_default() += demand;
        }
        for &predecessor in &activity.predecessors {
            if predecessor >= count || predecessor == index {
                return Err(SimError(format!(
                    "Invalid predecessor index {predecessor} for activity {index}"
                )));
            }
            successors[predecessor].push(index);
            indegree[index] += 1;
        }
    }

    let mut ready = VecDeque::new();
    for (index, degree) in indegree.iter().enumerate() {
        if *degree == 0 {
            ready.push_back(index);
        }
    }
    let mut path_ticks = vec![0.0f64; count];
    let mut critical_path = 0.0f64;
    let mut visited = 0;
    while let Some(index) = ready.pop_front() {
        visited += 1;
        path_ticks[index] += activities[index].local_ticks;
        critical_path = critical_path.max(path_ticks[index]);
        for &successor in &successors[index] {
            path_ticks[successor] = path_ticks[successor].max(path_ticks[index]);
            indegree[successor] -= 1;
            if indegree[successor] == 0 {
                ready.push_back(successor);
            }
        }
    }
    if visited != count {
        return Err(SimError("Cyclic activity dependencies".into()));
    }

    Ok(engine_work
        .values()
        .copied()
        .fold(critical_path, f64::max)
        .max(resource_work.values().copied().fold(0.0, f64::max)))
}
