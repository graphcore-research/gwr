// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Generic task release constraints and native execution timestamps.
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::future::{Future, poll_fn};
use std::rc::Rc;
use std::task::Poll;

use async_trait::async_trait;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::task::Task;
use serde::{Deserialize, Serialize};

use crate::Timetable;

/// A task may start after both an absolute tick and its completion
/// prerequisites. `delay_ticks` is added once to the latest prerequisite
/// completion.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub not_before_tick: u64,
    pub after: Vec<String>,
    pub delay_ticks: u64,
}
/// Observed model ticks. Eligibility covers explicit release constraints only,
/// not data readiness. Unconstrained interior tasks have release tick zero.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTiming {
    pub eligible_tick: Option<u64>,
    pub started_tick: Option<u64>,
    pub completed_tick: Option<u64>,
}
/// Dispatch adapter that retains Timetable's resource and data scheduling.
pub struct ScheduledTimetable {
    timetable: Rc<Timetable>,
    clock: Clock,
    releases: BTreeMap<usize, Release>,
    ids: BTreeMap<String, usize>,
    timings: RefCell<Vec<TaskTiming>>,
}
impl ScheduledTimetable {
    /// Validate task IDs and the combined graph/release dependency DAG.
    pub fn new(
        timetable: Rc<Timetable>,
        clock: Clock,
        releases: BTreeMap<String, Release>,
    ) -> Result<Self, SimError> {
        let ids = timetable
            .graph
            .nodes()
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_task())
            .map(|(i, n)| (n.id().to_string(), i))
            .collect::<BTreeMap<_, _>>();
        let mut indexed = BTreeMap::new();
        let mut predecessors = timetable
            .graph
            .nodes()
            .iter()
            .map(|n| n.predecessors().to_vec())
            .collect::<Vec<_>>();
        for (id, release) in releases {
            let index = *ids
                .get(&id)
                .ok_or_else(|| SimError(format!("unknown release task {id}")))?;
            if release.after.is_empty() && release.delay_ticks != 0 {
                return Err(SimError("a completion delay requires a predecessor".into()));
            }
            for parent in &release.after {
                predecessors[index].push(*ids.get(parent).ok_or_else(|| {
                    SimError(format!("unknown completion prerequisite {parent}"))
                })?);
            }
            indexed.insert(index, release);
        }
        let mut degrees = predecessors.iter().map(Vec::len).collect::<Vec<_>>();
        let mut successors = vec![vec![]; predecessors.len()];
        for (i, parents) in predecessors.iter().enumerate() {
            for parent in parents {
                successors[*parent].push(i);
            }
        }
        let mut ready = degrees
            .iter()
            .enumerate()
            .filter_map(|(i, n)| (*n == 0).then_some(i))
            .collect::<VecDeque<_>>();
        let mut done = 0;
        while let Some(index) = ready.pop_front() {
            done += 1;
            for successor in &successors[index] {
                degrees[*successor] -= 1;
                if degrees[*successor] == 0 {
                    ready.push_back(*successor);
                }
            }
        }
        if done != predecessors.len() {
            return Err(SimError(
                "release constraints create a dependency cycle".into(),
            ));
        }
        let timings = vec![TaskTiming::default(); timetable.graph.nodes().len()];
        Ok(Self {
            timetable,
            clock,
            releases: indexed,
            ids,
            timings: RefCell::new(timings),
        })
    }
    /// Snapshot native task timestamps, suitable for request/token boundaries.
    pub fn timings(&self) -> BTreeMap<String, TaskTiming> {
        let times = self.timings.borrow();
        self.ids
            .iter()
            .map(|(id, index)| (id.clone(), times[*index].clone()))
            .collect()
    }
    /// Read one boundary timestamp without cloning the whole graph snapshot.
    pub fn timing(&self, id: &str) -> Result<TaskTiming, SimError> {
        let index = *self
            .ids
            .get(id)
            .ok_or_else(|| SimError(format!("unknown timing task {id}")))?;
        Ok(self.timings.borrow()[index].clone())
    }
    /// Earliest task eligibility from release constraints, before PE queueing.
    pub fn eligibility_tick(&self, id: &str) -> Result<Option<u64>, SimError> {
        let index = *self
            .ids
            .get(id)
            .ok_or_else(|| SimError(format!("unknown timing task {id}")))?;
        self.due(index)
    }
    fn due(&self, index: usize) -> Result<Option<u64>, SimError> {
        let Some(release) = self.releases.get(&index) else {
            return Ok(Some(0));
        };
        let times = self.timings.borrow();
        let mut latest = 0;
        for parent in &release.after {
            let Some(completed) = times[self.ids[parent]].completed_tick else {
                return Ok(None);
            };
            latest = latest.max(completed);
        }
        let relative = latest
            .checked_add(release.delay_ticks)
            .ok_or_else(|| SimError("release time overflows".into()))?;
        Ok(Some(release.not_before_tick.max(relative)))
    }
    fn next_timer(&self) -> Option<u64> {
        let now = self.clock.tick_now().tick();
        let mut earliest = None;
        for pe in self.timetable.nodes_per_pe.keys() {
            let ready = self
                .timetable
                .ready_nodes_per_pe
                .borrow()
                .get(pe)
                .cloned()
                .unwrap_or_default();
            for index in ready {
                if let Ok(Some(due)) = self.due(index)
                    && due > now
                {
                    earliest = Some(earliest.map_or(due, |old: u64| old.min(due)));
                }
            }
        }
        earliest
    }
}
#[async_trait(?Send)]
impl Dispatch for ScheduledTimetable {
    fn task_by_id(&self, index: usize) -> Result<Task, SimError> {
        self.timetable.task_by_id(index)
    }
    fn ready_task_indices(&self, pe: &str) -> Result<(bool, Vec<usize>), SimError> {
        let (done, ready) = self.timetable.ready_task_indices(pe)?;
        let now = self.clock.tick_now().tick();
        let mut released = vec![];
        for index in ready {
            if let Some(due) = self.due(index)?
                && due <= now
            {
                released.push(index);
                self.timings.borrow_mut()[index].eligible_tick = Some(due);
            }
        }
        Ok((done, released))
    }
    fn set_task_active(&self, index: usize) -> SimResult {
        let due = self
            .due(index)?
            .ok_or_else(|| SimError("unreleased task activated".into()))?;
        if due > self.clock.tick_now().tick() {
            return Err(SimError("task activated before release".into()));
        }
        let mut timings = self.timings.borrow_mut();
        let time = &mut timings[index];
        time.eligible_tick = Some(due);
        time.started_tick = Some(self.clock.tick_now().tick());
        drop(timings);
        self.timetable.set_task_active(index)
    }
    fn set_task_completed(&self, index: usize) -> SimResult {
        self.timings.borrow_mut()[index].completed_tick = Some(self.clock.tick_now().tick());
        self.timetable.set_task_completed(index)
    }
    async fn wait_for_change(&self) {
        let mut changed = Box::pin(self.timetable.wait_for_change());
        if let Some(due) = self.next_timer() {
            let mut timer = Box::pin(
                self.clock
                    .wait_ticks(due - self.clock.tick_now().tick())
                    .expect("relative wait ends at the already representable due tick"),
            );
            poll_fn(|cx| {
                if changed.as_mut().poll(cx).is_ready() || timer.as_mut().poll(cx).is_ready() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
        } else {
            changed.await;
        }
    }
    fn total_tasks_for_pe(&self, pe: &str) -> usize {
        self.timetable.total_tasks_for_pe(pe)
    }
}
