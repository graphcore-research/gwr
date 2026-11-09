// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Open admission of validated immutable graphs into one native dispatcher.
//! Drained graphs may retire; actual branded completion facts survive when
//! retained by callers. Storage freeing/distributed aliases remain unsupported.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::future::{Future, poll_fn};
use std::rc::{Rc, Weak};
use std::task::Poll;

use async_trait::async_trait;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::task::Task;
use gwr_models::processing_element::tensors::TensorView;
use gwr_platform::Platform;

use crate::Timetable;
use crate::scheduled::TaskTiming;

/// Release of every task in a segment. Parents are actual previously published
/// global task IDs; delay is applied once after the latest completion.
#[derive(Clone, Debug, Default)]
pub struct SegmentRelease {
    pub not_before_tick: u64,
    pub after: Vec<usize>,
    pub delay_ticks: u64,
}
/// Committed task handles, unique across all segments in this dispatcher.
#[derive(Debug)]
pub struct PublishedSegment {
    pub tasks: BTreeMap<String, usize>,
    key: Rc<()>,
    handles: BTreeMap<String, PublishedTaskHandle>,
}
impl PublishedSegment {
    /// Actual published task handles, prepared before the atomic commit.
    #[must_use]
    pub fn handles(&self) -> &BTreeMap<String, PublishedTaskHandle> {
        &self.handles
    }
}
/// Exact native task placement/category, without cloning its full program.
#[derive(Clone, Debug)]
pub struct PublishedTaskMetadata {
    pub local_id: String,
    pub processing_element: String,
    pub is_compute: bool,
}
/// Owner-branded published task event. Private construction prevents a numeric
/// ID from another dispatcher being mistaken for this dispatcher's event.
#[derive(Clone, Debug)]
pub struct PublishedTaskHandle {
    owner: Rc<()>,
    fact: Rc<TaskFact>,
}
#[derive(Debug)]
struct TaskFact {
    id: usize,
    timing: RefCell<TaskTiming>,
    metadata: PublishedTaskMetadata,
}
/// Immutable certification of an actual dispatcher completion. Construction
/// requires a real completed transition; callers cannot invent a timestamp.
#[derive(Clone, Debug)]
pub struct CompletedTaskFact {
    handle: PublishedTaskHandle,
    completed_tick: u64,
}
impl CompletedTaskFact {
    /// Original monotone native task identity.
    #[must_use]
    pub fn id(&self) -> usize {
        self.handle.id()
    }
    /// Tick written by the actual native completion callback.
    #[must_use]
    pub fn completed_tick(&self) -> u64 {
        self.completed_tick
    }
    /// Durable branded prerequisite, valid after graph retirement.
    #[must_use]
    pub fn handle(&self) -> &PublishedTaskHandle {
        &self.handle
    }
}
impl PublishedTaskHandle {
    /// Numeric dispatch index, interpreted only within the owning dispatcher.
    #[must_use]
    pub fn id(&self) -> usize {
        self.fact.id
    }
}
/// Residency, durable facts and cumulative actual publication populations.
#[derive(Clone, Debug)]
pub struct SegmentStats {
    pub resident_nodes: usize,
    pub resident_tasks: usize,
    pub cumulative_published_nodes: usize,
    pub cumulative_published_tasks: usize,
    pub active_tasks: usize,
    pub completed_tasks: usize,
    pub admissions_closed: bool,
    pub resident_segments: usize,
    pub resident_parent_links: usize,
    pub resident_physical_accesses: usize,
    pub fact_registry_entries: usize,
    pub completed_fact_cells: usize,
    pub peak_resident_nodes: usize,
    pub peak_fact_registry_entries: usize,
}
struct Segment {
    timetable: Rc<Timetable>,
    base: usize,
    end: usize,
    release: StoredRelease,
    key: Rc<()>,
}
struct StoredRelease {
    not_before_tick: u64,
    after: Vec<PublishedTaskHandle>,
    delay_ticks: u64,
}
struct Access {
    task: usize,
    write: bool,
    view: TensorView,
}
#[derive(Default)]
struct State {
    segments: Vec<Segment>,
    next_id: usize,
    times: BTreeMap<usize, Rc<TaskFact>>,
    fact_registry: Vec<Weak<TaskFact>>,
    cumulative_tasks: usize,
    cumulative_completed: usize,
    tasks_per_pe: BTreeMap<String, usize>,
    peak_resident_nodes: usize,
    peak_fact_registry_entries: usize,
    parents: BTreeMap<usize, Vec<usize>>,
    accesses: Vec<Access>,
    closed: bool,
    failure: Option<String>,
}
/// Single-engine segment admission. The producer must remain an engine future
/// while admissions are open; closing admissions drains existing tasks.
/// Physical hazard checks currently require one memory address space. Resident
/// graphs retire explicitly after all native tasks drain. Durable facts remain
/// only while resident dependencies or callers keep them. Storage is not freed.
pub struct SegmentDispatcher {
    platform: Rc<Platform>,
    clock: Clock,
    max_nodes: usize,
    state: RefCell<State>,
    changed: Repeated<()>,
    owner: Rc<()>,
}
impl SegmentDispatcher {
    /// Construct an initially empty, open dispatcher on an existing platform.
    #[must_use]
    pub fn new(platform: Rc<Platform>, clock: Clock, max_resident_nodes: usize) -> Self {
        Self {
            platform,
            clock,
            max_nodes: max_resident_nodes,
            state: RefCell::new(State::default()),
            changed: Repeated::default(),
            owner: Rc::new(()),
        }
    }
    /// Validate the entire segment before committing handles or graph state.
    /// Requires the same platform, preflight success, known prior completion
    /// events, available node capacity and ordered physical read/write hazards.
    pub fn publish(
        &self,
        timetable: Rc<Timetable>,
        release: SegmentRelease,
    ) -> Result<PublishedSegment, SimError> {
        self.publish_boundaries(timetable, release, &[])
            .map(|(published, _)| published)
    }
    /// Resolve ordered graph-local boundary task names before any publication.
    /// Unknown names, tensor nodes and duplicate partitions reject atomically.
    pub fn publish_boundaries(
        &self,
        timetable: Rc<Timetable>,
        release: SegmentRelease,
        boundaries: &[String],
    ) -> Result<(PublishedSegment, Vec<usize>), SimError> {
        self.publish_boundaries_handles(timetable, release, boundaries)
            .map(|(published, handles)| {
                (
                    published,
                    handles.iter().map(PublishedTaskHandle::id).collect(),
                )
            })
    }
    /// Atomically resolve named boundary events into owner-branded handles.
    /// Handles are constructed before publication and returned only on commit.
    pub fn publish_boundaries_handles(
        &self,
        timetable: Rc<Timetable>,
        release: SegmentRelease,
        boundaries: &[String],
    ) -> Result<(PublishedSegment, Vec<PublishedTaskHandle>), SimError> {
        self.publish_boundaries_after(timetable, release, boundaries, &[])
    }
    /// Publish with owner-branded actual prerequisites. Completed facts remain
    /// usable after retirement. Raw numeric prerequisites are resident-only.
    #[expect(
        clippy::too_many_lines,
        reason = "All publication validation and handle preparation must precede the single atomic state commit."
    )]
    pub fn publish_boundaries_after(
        &self,
        timetable: Rc<Timetable>,
        release: SegmentRelease,
        boundaries: &[String],
        prerequisites: &[PublishedTaskHandle],
    ) -> Result<(PublishedSegment, Vec<PublishedTaskHandle>), SimError> {
        let s = self.state.borrow();
        Self::check_error(&s)?;
        if s.closed {
            return Err(SimError("segment admissions closed".into()));
        }
        if !Rc::ptr_eq(&timetable.platform, &self.platform) {
            return Err(SimError("segment belongs to another platform".into()));
        }
        if self.platform.num_memories() != 1 {
            return Err(SimError(
                "segmented D1 requires one memory address space; distributed aliases unsupported"
                    .into(),
            ));
        }
        if release.after.is_empty() && prerequisites.is_empty() && release.delay_ticks != 0 {
            return Err(SimError("completion delay requires a parent".into()));
        }
        if release.after.iter().any(|p| !s.times.contains_key(p)) {
            return Err(SimError("unknown segment completion parent".into()));
        }
        if release.after.iter().collect::<BTreeSet<_>>().len() != release.after.len() {
            return Err(SimError("duplicate segment completion parent".into()));
        }
        let mut parent_handles = release
            .after
            .into_iter()
            .map(|id| PublishedTaskHandle {
                owner: self.owner.clone(),
                fact: s.times[&id].clone(),
            })
            .collect::<Vec<_>>();
        for handle in prerequisites {
            self.timing_handle(handle)?;
            if parent_handles.iter().any(|h| h.id() == handle.id()) {
                return Err(SimError("duplicate segment completion parent".into()));
            }
            parent_handles.push(handle.clone());
        }
        let parent_ids = parent_handles
            .iter()
            .map(PublishedTaskHandle::id)
            .collect::<Vec<_>>();
        if !timetable.active_node_indices.borrow().is_empty()
            || timetable
                .completed_node_indices
                .borrow()
                .iter()
                .any(|i| timetable.graph.nodes()[*i].is_task())
            || s.segments
                .iter()
                .any(|segment| Rc::ptr_eq(&segment.timetable, &timetable))
        {
            return Err(SimError("segment graph already used or published".into()));
        }
        timetable.preflight()?;
        let nodes = timetable.graph.nodes();
        if !nodes.iter().any(|n| n.is_task()) {
            return Err(SimError("segment has no tasks".into()));
        }
        let end = s
            .next_id
            .checked_add(nodes.len())
            .ok_or_else(|| SimError("global segment ID overflow".into()))?;
        let resident = s
            .segments
            .iter()
            .map(|segment| segment.end - segment.base)
            .sum::<usize>()
            .checked_add(nodes.len())
            .ok_or_else(|| SimError("resident node count overflow".into()))?;
        if resident > self.max_nodes {
            return Err(SimError("resident segment node budget exceeded".into()));
        }
        let mut tasks = BTreeMap::new();
        let mut accesses = vec![];
        let mut parents = BTreeMap::new();
        let mut nearest = vec![vec![]; nodes.len()];
        for &i in timetable.graph.topological_order() {
            let mut incoming = parent_ids.clone();
            for p in nodes[i].predecessors() {
                incoming.extend(nearest[*p].iter().copied());
            }
            incoming.sort_unstable();
            incoming.dedup();
            if nodes[i].is_task() {
                let id = s.next_id + i;
                tasks.insert(nodes[i].id().to_string(), id);
                parents.insert(id, incoming);
                nearest[i] = vec![id];
                let views = timetable
                    .graph
                    .compute_views(i)
                    .ok_or_else(|| SimError("segment task lacks views".into()))?;
                for (write, values) in [(false, views.inputs()), (true, views.outputs())] {
                    for view in values.iter().flatten() {
                        accesses.push(Access {
                            task: id,
                            write,
                            view: view.clone(),
                        });
                    }
                }
            } else {
                nearest[i] = incoming;
            }
        }
        if boundaries.iter().collect::<BTreeSet<_>>().len() != boundaries.len() {
            return Err(SimError("duplicate segment boundary".into()));
        }
        let mapped = boundaries
            .iter()
            .map(|id| {
                tasks
                    .get(id)
                    .copied()
                    .ok_or_else(|| SimError(format!("unknown or non-task segment boundary {id}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for a in &accesses {
            for b in &s.accesses {
                if (a.write || b.write)
                    && a.view.first_overlapping_byte_ranges(&b.view).is_some()
                    && s.times[&b.task].timing.borrow().completed_tick.is_none()
                    && !Self::precedes(b.task, &parent_ids, &s.parents)
                {
                    return Err(SimError(format!(
                        "unordered cross-segment storage hazard with task {}",
                        b.task
                    )));
                }
            }
        }
        let cumulative_tasks = s
            .cumulative_tasks
            .checked_add(tasks.len())
            .ok_or_else(|| SimError("cumulative task count overflow".into()))?;
        let mut tasks_per_pe = s.tasks_per_pe.clone();
        let mut handles = BTreeMap::new();
        for (name, id) in &tasks {
            let node = &nodes[*id - s.next_id];
            let pe = node
                .pe()
                .ok_or_else(|| SimError("published task lacks PE".into()))?
                .to_string();
            let count = tasks_per_pe.entry(pe.clone()).or_default();
            *count = count
                .checked_add(1)
                .ok_or_else(|| SimError("cumulative PE task count overflow".into()))?;
            handles.insert(
                name.clone(),
                PublishedTaskHandle {
                    owner: self.owner.clone(),
                    fact: Rc::new(TaskFact {
                        id: *id,
                        timing: RefCell::new(TaskTiming::default()),
                        metadata: PublishedTaskMetadata {
                            local_id: name.clone(),
                            processing_element: pe,
                            is_compute: node.is_compute(),
                        },
                    }),
                },
            );
        }
        let by_id = handles
            .values()
            .map(|handle| (handle.id(), handle.clone()))
            .collect::<BTreeMap<_, _>>();
        let mapped = mapped.into_iter().map(|id| by_id[&id].clone()).collect();
        let base = s.next_id;
        let key = Rc::new(());
        let stored_release = StoredRelease {
            not_before_tick: release.not_before_tick,
            after: parent_handles,
            delay_ticks: release.delay_ticks,
        };
        drop(s);
        let mut s = self.state.borrow_mut();
        s.fact_registry.retain(|fact| fact.strong_count() > 0);
        for handle in handles.values() {
            s.fact_registry.push(Rc::downgrade(&handle.fact));
            s.times.insert(handle.id(), handle.fact.clone());
        }
        s.cumulative_tasks = cumulative_tasks;
        s.tasks_per_pe = tasks_per_pe;
        s.peak_resident_nodes = s.peak_resident_nodes.max(resident);
        s.peak_fact_registry_entries = s.peak_fact_registry_entries.max(s.fact_registry.len());
        s.parents.extend(parents);
        s.accesses.extend(accesses);
        s.segments.push(Segment {
            timetable,
            base,
            end,
            release: stored_release,
            key: key.clone(),
        });
        s.next_id = end;
        drop(s);
        self.changed.notify();
        Ok((
            PublishedSegment {
                tasks,
                key,
                handles,
            },
            mapped,
        ))
    }
    fn precedes(task: usize, roots: &[usize], parents: &BTreeMap<usize, Vec<usize>>) -> bool {
        let mut todo = roots.to_vec();
        let mut seen = BTreeSet::new();
        while let Some(id) = todo.pop() {
            if id == task {
                return true;
            }
            if seen.insert(id) {
                todo.extend(parents.get(&id).into_iter().flatten().copied());
            }
        }
        false
    }
    /// Prohibit publication and wake PE runners; outstanding tasks still drain.
    pub fn close(&self) -> SimResult {
        let mut s = self.state.borrow_mut();
        Self::check_error(&s)?;
        s.closed = true;
        drop(s);
        self.changed.notify();
        Ok(())
    }
    /// Record a producer failure and wake all runners. They return this error
    /// through the engine; failure is never a successful completion event.
    pub fn fail(&self, message: impl Into<String>) -> SimResult {
        let mut s = self.state.borrow_mut();
        if s.failure.is_none() {
            s.failure = Some(message.into());
        }
        drop(s);
        self.changed.notify();
        Ok(())
    }
    /// Report both residency and cumulative populations. Open-stream PE task
    /// totals are diagnostics; they never determine admission closure.
    pub fn stats(&self) -> SegmentStats {
        let mut s = self.state.borrow_mut();
        s.fact_registry.retain(|fact| fact.strong_count() > 0);
        SegmentStats {
            resident_nodes: s.segments.iter().map(|x| x.end - x.base).sum(),
            resident_tasks: s.times.len(),
            cumulative_published_nodes: s.next_id,
            cumulative_published_tasks: s.cumulative_tasks,
            active_tasks: s
                .times
                .values()
                .filter(|t| {
                    let t = t.timing.borrow();
                    t.started_tick.is_some() && t.completed_tick.is_none()
                })
                .count(),
            completed_tasks: s.cumulative_completed,
            admissions_closed: s.closed,
            resident_segments: s.segments.len(),
            resident_parent_links: s.parents.values().map(Vec::len).sum(),
            resident_physical_accesses: s.accesses.len(),
            fact_registry_entries: s.fact_registry.len(),
            completed_fact_cells: s
                .fact_registry
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|f| f.timing.borrow().completed_tick.is_some())
                .count(),
            peak_resident_nodes: s.peak_resident_nodes,
            peak_fact_registry_entries: s.peak_fact_registry_entries,
        }
    }
    /// Create a handle only for a real resident task. Retired numeric IDs are
    /// unavailable; callers retain actual typed handles before retirement.
    pub fn task_handle(&self, id: usize) -> Result<PublishedTaskHandle, SimError> {
        let s = self.state.borrow();
        Self::check_error(&s)?;
        Ok(PublishedTaskHandle {
            owner: self.owner.clone(),
            fact: s
                .times
                .get(&id)
                .cloned()
                .ok_or_else(|| SimError("unknown resident task handle".into()))?,
        })
    }
    /// Owner validation precedes reading the private actual transition cell.
    /// Handles remain valid after their completed graph retires.
    pub fn timing_handle(&self, handle: &PublishedTaskHandle) -> Result<TaskTiming, SimError> {
        if !Rc::ptr_eq(&self.owner, &handle.owner) {
            return Err(SimError(
                "published task belongs to another dispatcher".into(),
            ));
        }
        Self::check_error(&self.state.borrow())?;
        Ok(handle.fact.timing.borrow().clone())
    }
    /// Certify only an actual completed task; no caller-provided clock value is
    /// accepted. The terminal fact retains the original private task cell.
    pub fn completed_fact(
        &self,
        handle: &PublishedTaskHandle,
    ) -> Result<CompletedTaskFact, SimError> {
        let completed_tick = self
            .timing_handle(handle)?
            .completed_tick
            .ok_or_else(|| SimError("task has not actually completed".into()))?;
        Ok(CompletedTaskFact {
            handle: handle.clone(),
            completed_tick,
        })
    }
    /// Retire one privately identified fully drained graph. Unrelated graphs
    /// may remain active. Failure leaves all graph/fact/counter state
    /// unchanged.
    pub fn retire_segment(&self, published: &PublishedSegment) -> SimResult {
        self.retire_keys(std::slice::from_ref(&published.key))
            .map(|_| ())
    }
    /// Retire all currently drained graphs, retaining pending physical hazards
    /// and durable facts held by callers/remaining releases. Storage is not
    /// freed.
    pub fn retire_drained(&self) -> Result<usize, SimError> {
        let keys = {
            let s = self.state.borrow();
            Self::check_error(&s)?;
            s.segments
                .iter()
                .filter(|segment| Self::segment_drained(&s, segment))
                .map(|segment| segment.key.clone())
                .collect::<Vec<_>>()
        };
        self.retire_keys(&keys)
    }
    fn segment_drained(s: &State, segment: &Segment) -> bool {
        s.times
            .range(segment.base..segment.end)
            .all(|(_, fact)| fact.timing.borrow().completed_tick.is_some())
    }
    fn retire_keys(&self, keys: &[Rc<()>]) -> Result<usize, SimError> {
        let mut s = self.state.borrow_mut();
        Self::check_error(&s)?;
        if keys
            .iter()
            .enumerate()
            .any(|(i, key)| keys[..i].iter().any(|prior| Rc::ptr_eq(key, prior)))
        {
            return Err(SimError("duplicate retirement candidate".into()));
        }
        for key in keys {
            let segment = s
                .segments
                .iter()
                .find(|segment| Rc::ptr_eq(&segment.key, key))
                .ok_or_else(|| SimError("unknown or retired segment".into()))?;
            if !Self::segment_drained(&s, segment)
                || !segment.timetable.active_node_indices.borrow().is_empty()
            {
                return Err(SimError(
                    "retirement candidate has unfinished native work".into(),
                ));
            }
            for pe in &self.platform.pe_names() {
                let (_, ready) = segment.timetable.ready_task_indices(pe)?;
                if !ready.is_empty() {
                    return Err(SimError("retirement candidate has ready work".into()));
                }
            }
        }
        let ranges = s
            .segments
            .iter()
            .filter(|segment| keys.iter().any(|key| Rc::ptr_eq(key, &segment.key)))
            .map(|segment| (segment.base, segment.end))
            .collect::<Vec<_>>();
        let retired = |id: usize| ranges.iter().any(|(start, end)| *start <= id && id < *end);
        s.times.retain(|id, _| !retired(*id));
        s.parents.retain(|id, _| !retired(*id));
        s.accesses.retain(|access| !retired(access.task));
        s.segments
            .retain(|segment| !keys.iter().any(|key| Rc::ptr_eq(key, &segment.key)));
        s.fact_registry.retain(|fact| fact.strong_count() > 0);
        drop(s);
        self.changed.notify();
        Ok(keys.len())
    }
    /// Capture the change generation synchronously before checking producer
    /// completion/readiness. A change before polling the future is retained.
    pub fn subscribe_changes(&self) -> gwr_engine::traits::BoxFuture<'static, ()> {
        self.changed.listen()
    }
    /// Inspect an actually published task's owner PE and native category.
    pub fn task_metadata(&self, id: usize) -> Result<PublishedTaskMetadata, SimError> {
        let s = self.state.borrow();
        Self::check_error(&s)?;
        s.times
            .get(&id)
            .map(|f| f.metadata.clone())
            .ok_or_else(|| SimError("unknown resident task metadata".into()))
    }
    /// Actual resident task timing. Retired numeric IDs are unavailable.
    pub fn timing(&self, id: usize) -> Result<TaskTiming, SimError> {
        let s = self.state.borrow();
        Self::check_error(&s)?;
        s.times
            .get(&id)
            .map(|f| f.timing.borrow().clone())
            .ok_or_else(|| SimError("unknown resident task timing".into()))
    }
    /// Snapshot only resident native timestamps, never an unbounded archive.
    pub fn timings(&self) -> BTreeMap<usize, TaskTiming> {
        self.state
            .borrow()
            .times
            .iter()
            .map(|(id, f)| (*id, f.timing.borrow().clone()))
            .collect()
    }
    fn check_error(s: &State) -> SimResult {
        if let Some(e) = &s.failure {
            Err(SimError(e.clone()))
        } else {
            Ok(())
        }
    }
    fn due(r: &StoredRelease) -> Result<Option<u64>, SimError> {
        let mut latest = 0;
        for handle in &r.after {
            let Some(t) = handle.fact.timing.borrow().completed_tick else {
                return Ok(None);
            };
            latest = latest.max(t);
        }
        Ok(Some(
            r.not_before_tick.max(
                latest
                    .checked_add(r.delay_ticks)
                    .ok_or_else(|| SimError("segment release overflow".into()))?,
            ),
        ))
    }
    fn route(s: &State, id: usize) -> Result<(&Segment, usize), SimError> {
        let segment = s
            .segments
            .iter()
            .find(|x| x.base <= id && id < x.end)
            .ok_or_else(|| SimError("unknown global segment task".into()))?;
        if !s.times.contains_key(&id) {
            return Err(SimError("global ID is not a task".into()));
        }
        Ok((segment, id - segment.base))
    }
    fn timer(&self, pe: &str) -> Result<Option<u64>, SimError> {
        let s = self.state.borrow();
        Self::check_error(&s)?;
        let now = self.clock.tick_now().tick();
        let mut earliest = None;
        for segment in &s.segments {
            if let Some(due) = Self::due(&segment.release)?
                && due > now
                && !segment.timetable.ready_task_indices(pe)?.1.is_empty()
            {
                earliest = Some(earliest.map_or(due, |x: u64| x.min(due)));
            }
        }
        Ok(earliest)
    }
}
#[async_trait(?Send)]
impl Dispatch for SegmentDispatcher {
    fn task_by_id(&self, id: usize) -> Result<Task, SimError> {
        let s = self.state.borrow();
        Self::check_error(&s)?;
        let (segment, local) = Self::route(&s, id)?;
        segment.timetable.task_by_id(local)
    }
    fn set_task_active(&self, id: usize) -> SimResult {
        let s = self.state.borrow();
        Self::check_error(&s)?;
        let (segment, local) = Self::route(&s, id)?;
        let due = Self::due(&segment.release)?
            .ok_or_else(|| SimError("segment task unreleased".into()))?;
        let now = self.clock.tick_now().tick();
        if due > now || s.times[&id].timing.borrow().started_tick.is_some() {
            return Err(SimError(
                "segment task early or duplicate activation".into(),
            ));
        }
        segment.timetable.set_task_active(local)?;
        let mut time = s.times[&id].timing.borrow_mut();
        time.eligible_tick = Some(due);
        time.started_tick = Some(now);
        drop(time);
        drop(s);
        self.changed.notify();
        Ok(())
    }
    fn set_task_completed(&self, id: usize) -> SimResult {
        let mut s = self.state.borrow_mut();
        Self::check_error(&s)?;
        let (segment, local) = Self::route(&s, id)?;
        let time = s.times[&id].timing.borrow();
        if time.started_tick.is_none() || time.completed_tick.is_some() {
            return Err(SimError("segment completion without active task".into()));
        }
        let total = s
            .cumulative_completed
            .checked_add(1)
            .ok_or_else(|| SimError("cumulative completion count overflow".into()))?;
        drop(time);
        segment.timetable.set_task_completed(local)?;
        s.times[&id].timing.borrow_mut().completed_tick = Some(self.clock.tick_now().tick());
        s.cumulative_completed = total;
        drop(s);
        self.changed.notify();
        Ok(())
    }
    fn ready_task_indices(&self, pe: &str) -> Result<(bool, Vec<usize>), SimError> {
        self.platform.pe_idx_from_name(pe)?;
        let s = self.state.borrow();
        Self::check_error(&s)?;
        let mut done = s.closed;
        let mut ready = vec![];
        let now = self.clock.tick_now().tick();
        for segment in &s.segments {
            let (local_done, local_ready) = segment.timetable.ready_task_indices(pe)?;
            done &= local_done;
            if Self::due(&segment.release)?.is_some_and(|due| due <= now) {
                ready.extend(local_ready.into_iter().map(|i| segment.base + i));
            }
        }
        Ok((done, ready))
    }
    async fn wait_for_change(&self) {
        self.changed.listen().await;
    }
    async fn wait_for_ready(&self, pe: &str) -> SimResult {
        loop {
            // Register before the readiness/timer checks. In the single-thread
            // engine no mutation can intervene without a future yield.
            let mut changed = self.changed.listen();
            let (done, ready) = self.ready_task_indices(pe)?;
            if done || !ready.is_empty() {
                return Ok(());
            }
            let mut timer = self.timer(pe)?.map(|due| {
                Box::pin(
                    self.clock
                        .wait_ticks(due - self.clock.tick_now().tick())
                        .expect("relative wait ends at the already representable due tick"),
                )
            });
            poll_fn(|cx| {
                if changed.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(());
                }
                if let Some(t) = timer.as_mut()
                    && t.as_mut().poll(cx).is_ready()
                {
                    return Poll::Ready(());
                }
                Poll::Pending
            })
            .await;
        }
    }
    fn total_tasks_for_pe(&self, pe: &str) -> usize {
        self.state
            .borrow()
            .tasks_per_pe
            .get(pe)
            .copied()
            .unwrap_or(0)
    }
}
