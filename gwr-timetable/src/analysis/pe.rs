// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimError;
use gwr_models::processing_element::operators::{MachineOp, MachineOps};
use gwr_platform::Platform;
use log::debug;

use crate::analysis::memory::{
    BandwidthGraph, MemoryContentionAnalysis, MemoryContentionWindow, WidestPathCache,
};
use crate::analysis::{ComputeNodeAnalysis, format_bytes, ticks_to_ns};
use crate::format_machine_ops;

const NUM_SCHEDULE_ATTEMPTS: usize = 10;

#[derive(Clone, Debug)]
pub struct ComputeNodeRoofline {
    pub analysis: ComputeNodeAnalysis,
    pub bytes_by_memory: BTreeMap<String, usize>,
    pub compute_ticks: f64,
    pub memory_ticks: f64,
    pub roofline_ticks: f64,
}

impl ComputeNodeRoofline {
    #[must_use]
    pub fn memory_bytes(&self, memory_name: &str) -> usize {
        self.bytes_by_memory.get(memory_name).copied().unwrap_or(0)
    }
}

#[derive(Clone, Debug)]
pub struct PeRooflineSummary {
    pub pe_name: String,
    pub compute_nodes: usize,
    pub total_flops: usize,
    pub total_bytes: usize,
    pub bytes_by_memory: BTreeMap<String, usize>,
    pub compute_ticks: f64,
    pub memory_ticks: f64,
    pub roofline_ticks: f64,
}

#[derive(Clone, Debug)]
pub struct PeActivity {
    pub node: ComputeNodeRoofline,
    pub pe_name: String,
    pub base_memory_ticks_by_memory: BTreeMap<String, f64>,
    pub adjusted_memory_ticks_by_memory: BTreeMap<String, f64>,
    pub start_ticks: f64,
    pub end_ticks: f64,
}

impl PeActivity {
    #[must_use]
    pub fn base_memory_ticks(&self, memory_name: &str) -> f64 {
        self.base_memory_ticks_by_memory
            .get(memory_name)
            .copied()
            .unwrap_or(0.0)
    }

    #[must_use]
    pub fn deps_string(&self) -> String {
        let deps = if self.node.analysis.predecessor_compute_node_ids.is_empty() {
            "none".to_string()
        } else {
            self.node.analysis.predecessor_compute_node_ids.join(", ")
        };
        format!(" deps=[{deps}]")
    }

    #[must_use]
    pub fn memory_summary_string(&self, memory_name: &str) -> Option<String> {
        let duration_ticks = self.end_ticks - self.start_ticks;
        let base_memory_ticks = self.base_memory_ticks(memory_name);
        if duration_ticks <= 0.0 || base_memory_ticks <= 0.0 {
            return None;
        }
        let fraction = base_memory_ticks / duration_ticks;
        Some(format!(
            "{}@{} ({:.0}%)",
            self.node.analysis.id,
            self.pe_name,
            fraction * 100.0
        ))
    }

    #[must_use]
    pub fn print_report(&self, clock: &Clock, previous_end_ticks: f64, show_deps: bool) -> f64 {
        if self.start_ticks > previous_end_ticks {
            println!(
                "    {:.2}ns ({:.2}ns -> {:.2}ns) | IDLE",
                ticks_to_ns(clock, self.start_ticks - previous_end_ticks),
                ticks_to_ns(clock, previous_end_ticks),
                ticks_to_ns(clock, self.start_ticks),
            );
        }
        let base_memory_ticks = self.base_memory_ticks_by_memory.values().sum::<f64>();
        let adjusted_memory_ticks = self.adjusted_memory_ticks_by_memory.values().sum::<f64>();
        let memory_breakdown = if self.adjusted_memory_ticks_by_memory.is_empty() {
            "none".to_string()
        } else {
            self.adjusted_memory_ticks_by_memory
                .iter()
                .map(|(memory_name, ticks)| {
                    let base_ticks = self.base_memory_ticks(memory_name);
                    format!(
                        "{memory_name}: {:.2}ns->{:.2}ns",
                        ticks_to_ns(clock, base_ticks),
                        ticks_to_ns(clock, *ticks)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let start_ns = ticks_to_ns(clock, self.start_ticks);
        let end_ns = ticks_to_ns(clock, self.end_ticks);
        let duration_ns = end_ns - start_ns;
        let deps_suffix = if show_deps {
            self.deps_string()
        } else {
            String::new()
        };

        println!(
            "    {:.2}ns ({:.2}ns -> {:.2}ns) | {} compute={:.2}ns memory={:.2}ns->{:.2}ns bytes={} ({:.2}) flops={} [{}] mem=[{}]{}",
            duration_ns,
            start_ns,
            end_ns,
            self.node.analysis.id,
            ticks_to_ns(clock, self.node.compute_ticks),
            ticks_to_ns(clock, base_memory_ticks),
            ticks_to_ns(clock, adjusted_memory_ticks),
            self.node.analysis.total_bytes(),
            format_bytes(self.node.analysis.total_bytes()),
            self.node.analysis.flops,
            format_machine_ops(&self.node.analysis.machine_ops),
            memory_breakdown,
            deps_suffix
        );

        self.end_ticks
    }
}

pub struct ScheduledActivities {
    pub activities: Vec<PeActivity>,
    pub memory_analyses: BTreeMap<String, MemoryContentionAnalysis>,
}

pub struct CriticalPathAnalysis {
    pub total_ticks: f64,
    pub node_indices: Vec<usize>,
}

#[derive(Clone, Debug)]
struct ActivityModel {
    node: ComputeNodeRoofline,
    pe_name: String,
    base_memory_ticks_by_memory: BTreeMap<String, f64>,
}

impl ActivityModel {
    #[must_use]
    fn base_memory_ticks(&self, memory_name: &str) -> f64 {
        self.base_memory_ticks_by_memory
            .get(memory_name)
            .copied()
            .unwrap_or(0.0)
    }
}

#[derive(Clone, Debug)]
struct ActivityState {
    adjusted_memory_ticks_by_memory: BTreeMap<String, f64>,
    start_ticks: f64,
    end_ticks: f64,
}

impl ActivityState {
    #[must_use]
    fn adjusted_memory_ticks(&self, memory_name: &str) -> f64 {
        self.adjusted_memory_ticks_by_memory
            .get(memory_name)
            .copied()
            .unwrap_or(0.0)
    }
}

struct ScheduleContext {
    topological_order: Vec<usize>,
    activity_idx_by_node_idx: HashMap<usize, usize>,
    previous_on_pe: HashMap<usize, (String, Option<usize>)>,
}

#[derive(Clone, Copy)]
struct ActivityView<'a> {
    model: &'a ActivityModel,
    state: &'a ActivityState,
}

#[must_use]
fn ticks_for_bytes(bytes: usize, bandwidth: usize) -> f64 {
    if bytes > 0 {
        (bytes as f64 / bandwidth.max(1) as f64).ceil()
    } else {
        0.0
    }
}

pub fn compute_node_rooflines(
    platform: &Platform,
    compute_nodes: &[ComputeNodeAnalysis],
    bandwidth_graph: &BandwidthGraph,
    widest_path_cache: &mut WidestPathCache,
) -> Result<Vec<ComputeNodeRoofline>, Box<dyn std::error::Error>> {
    let mut rooflines = Vec::with_capacity(compute_nodes.len());

    for analysis in compute_nodes {
        let pe_name = analysis.pe_name.as_ref().ok_or_else(|| {
            SimError(format!(
                "Compute node '{}' is not assigned to a PE",
                analysis.id
            ))
        })?;
        let platform_pe = platform.pe(pe_name)?;
        let bytes_by_memory = analysis.bytes_by_memory.clone();

        let compute_ticks = platform_pe.compute_ticks_for_ops(&analysis.machine_ops)?;
        let mut memory_ticks = 0.0;
        for addr in &analysis.tensor_access_addrs {
            if !platform_pe.can_access_addr(*addr) {
                return Err(SimError(format!(
                    "Compute node '{}' on PE '{}' accesses address 0x{:x} which is not in the PE memory map",
                    analysis.id,
                    pe_name,
                    addr
                ))
                .into());
            }
        }
        for (memory_name, bytes) in &bytes_by_memory {
            let bandwidth = bandwidth_graph
                .cached_widest_path_bandwidth(
                    widest_path_cache,
                    &format!("pe:{pe_name}"),
                    &format!("mem:{memory_name}"),
                )
                .ok_or_else(|| {
                    SimError(format!(
                        "No bandwidth path from PE '{pe_name}' to memory '{memory_name}'"
                    ))
                })?;
            memory_ticks += ticks_for_bytes(*bytes, bandwidth);
        }

        let roofline_ticks = compute_ticks.max(memory_ticks);

        rooflines.push(ComputeNodeRoofline {
            analysis: analysis.clone(),
            bytes_by_memory,
            compute_ticks,
            memory_ticks,
            roofline_ticks,
        });
    }

    Ok(rooflines)
}

pub fn aggregate_pe_rooflines(
    platform: &Platform,
    node_rooflines: &[ComputeNodeRoofline],
    bandwidth_graph: &BandwidthGraph,
    widest_path_cache: &mut WidestPathCache,
) -> Result<Vec<PeRooflineSummary>, Box<dyn std::error::Error>> {
    let mut machine_ops_by_pe = HashMap::new();
    let mut flops_by_pe: HashMap<String, usize> = HashMap::new();
    let mut bytes_by_pe: HashMap<String, BTreeMap<String, usize>> = HashMap::new();
    let mut count_by_pe: HashMap<String, usize> = HashMap::new();

    for node in node_rooflines {
        let pe_name = node.analysis.pe_name.as_ref().ok_or_else(|| {
            SimError(format!(
                "Compute node '{}' is not assigned to a PE",
                node.analysis.id
            ))
        })?;
        let pe_machine_ops = machine_ops_by_pe
            .entry(pe_name.clone())
            .or_insert_with(MachineOps::new);
        for machine_op in MachineOp::ALL {
            if let Some(count) = node.analysis.machine_ops.get(&machine_op) {
                pe_machine_ops.add_op(machine_op, *count);
            }
        }
        *flops_by_pe.entry(pe_name.clone()).or_insert(0) += node.analysis.flops;
        *count_by_pe.entry(pe_name.clone()).or_insert(0) += 1;

        let pe_bytes = bytes_by_pe.entry(pe_name.clone()).or_default();
        for (memory_name, bytes) in &node.bytes_by_memory {
            *pe_bytes.entry(memory_name.clone()).or_insert(0) += *bytes;
        }
    }

    let mut summaries = Vec::new();
    for pe_name in platform.pe_names() {
        let platform_pe = platform.pe(&pe_name)?;
        let total_flops = flops_by_pe.get(&pe_name).copied().unwrap_or(0);
        let machine_ops = machine_ops_by_pe.remove(&pe_name).unwrap_or_default();
        let bytes_by_memory = bytes_by_pe.get(&pe_name).cloned().unwrap_or_default();
        let total_bytes = bytes_by_memory.values().sum();
        let compute_ticks = platform_pe.compute_ticks_for_ops(&machine_ops)?;
        let mut memory_ticks = 0.0;
        for (memory_name, bytes) in &bytes_by_memory {
            let bandwidth = bandwidth_graph
                .cached_widest_path_bandwidth(
                    widest_path_cache,
                    &format!("pe:{pe_name}"),
                    &format!("mem:{memory_name}"),
                )
                .ok_or_else(|| {
                    SimError(format!(
                        "No bandwidth path from PE '{pe_name}' to memory '{memory_name}'"
                    ))
                })?;
            memory_ticks += ticks_for_bytes(*bytes, bandwidth);
        }

        summaries.push(PeRooflineSummary {
            pe_name: pe_name.clone(),
            compute_nodes: count_by_pe.get(&pe_name).copied().unwrap_or(0),
            total_flops,
            total_bytes,
            bytes_by_memory,
            compute_ticks,
            memory_ticks,
            roofline_ticks: compute_ticks.max(memory_ticks),
        });
    }

    summaries.sort_by_key(|summary| Reverse(summary.total_flops));
    Ok(summaries)
}

fn base_memory_ticks_by_memory(
    pe_name: &str,
    bytes_by_memory: &BTreeMap<String, usize>,
    graph: &BandwidthGraph,
    widest_path_cache: &mut WidestPathCache,
) -> Result<BTreeMap<String, f64>, Box<dyn std::error::Error>> {
    let mut ticks_by_memory = BTreeMap::new();
    for (memory_name, bytes) in bytes_by_memory {
        let bandwidth = graph
            .cached_widest_path_bandwidth(
                widest_path_cache,
                &format!("pe:{pe_name}"),
                &format!("mem:{memory_name}"),
            )
            .ok_or_else(|| {
                SimError(format!(
                    "No bandwidth path from PE '{pe_name}' to memory '{memory_name}'"
                ))
            })?;
        let ticks = ticks_for_bytes(*bytes, bandwidth);
        ticks_by_memory.insert(memory_name.clone(), ticks);
    }
    Ok(ticks_by_memory)
}

fn topological_compute_node_order(node_rooflines: &[ComputeNodeRoofline]) -> Vec<usize> {
    let mut indegree = HashMap::new();
    let mut successors: HashMap<usize, Vec<usize>> = HashMap::new();

    for node in node_rooflines {
        indegree.insert(
            node.analysis.node_idx,
            node.analysis.predecessor_compute_node_indices.len(),
        );
        for predecessor in &node.analysis.predecessor_compute_node_indices {
            successors
                .entry(*predecessor)
                .or_default()
                .push(node.analysis.node_idx);
        }
    }

    let mut ready = BTreeSet::new();
    for (node_idx, degree) in &indegree {
        if *degree == 0 {
            ready.insert(*node_idx);
        }
    }

    let mut ordered = Vec::with_capacity(node_rooflines.len());
    while let Some(node_idx) = ready.pop_first() {
        ordered.push(node_idx);
        if let Some(next_nodes) = successors.get(&node_idx) {
            for next in next_nodes {
                let degree = indegree.get_mut(next).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(*next);
                }
            }
        }
    }

    ordered
}

fn order_nodes_per_pe(
    node_rooflines: &[ComputeNodeRoofline],
    topological_order: &[usize],
) -> BTreeMap<String, Vec<usize>> {
    let pe_by_node_idx = node_rooflines
        .iter()
        .filter_map(|node| {
            node.analysis
                .pe_name
                .as_ref()
                .map(|pe_name| (node.analysis.node_idx, pe_name.clone()))
        })
        .collect::<HashMap<_, _>>();

    let mut ordered = BTreeMap::new();
    for node_idx in topological_order {
        if let Some(pe_name) = pe_by_node_idx.get(node_idx) {
            ordered
                .entry(pe_name.clone())
                .or_insert_with(Vec::new)
                .push(*node_idx);
        }
    }
    ordered
}

fn build_activity_models(
    node_rooflines: &[ComputeNodeRoofline],
    graph: &BandwidthGraph,
    widest_path_cache: &mut WidestPathCache,
) -> Result<Vec<ActivityModel>, Box<dyn std::error::Error>> {
    let mut models = Vec::with_capacity(node_rooflines.len());
    for node in node_rooflines {
        let pe_name = node.analysis.pe_name.clone().ok_or_else(|| {
            SimError(format!(
                "Compute node '{}' is not assigned to a PE",
                node.analysis.id
            ))
        })?;
        let base_memory_ticks_by_memory =
            base_memory_ticks_by_memory(&pe_name, &node.bytes_by_memory, graph, widest_path_cache)?;
        models.push(ActivityModel {
            node: node.clone(),
            pe_name,
            base_memory_ticks_by_memory,
        });
    }
    Ok(models)
}

fn initialize_activity_states(models: &[ActivityModel]) -> Vec<ActivityState> {
    models
        .iter()
        .map(|model| ActivityState {
            adjusted_memory_ticks_by_memory: model.base_memory_ticks_by_memory.clone(),
            start_ticks: 0.0,
            end_ticks: 0.0,
        })
        .collect()
}

fn build_schedule_context(node_rooflines: &[ComputeNodeRoofline]) -> ScheduleContext {
    let topological_order = topological_compute_node_order(node_rooflines);
    let per_pe_order = order_nodes_per_pe(node_rooflines, &topological_order);
    let activity_idx_by_node_idx = node_rooflines
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.analysis.node_idx, idx))
        .collect::<HashMap<_, _>>();
    let previous_on_pe = per_pe_order
        .iter()
        .flat_map(|(pe_name, node_indices)| {
            node_indices
                .iter()
                .enumerate()
                .map(move |(position, node_idx)| {
                    let prev = if position == 0 {
                        None
                    } else {
                        Some(node_indices[position - 1])
                    };
                    (*node_idx, (pe_name.clone(), prev))
                })
        })
        .collect::<HashMap<_, _>>();

    ScheduleContext {
        topological_order,
        activity_idx_by_node_idx,
        previous_on_pe,
    }
}

fn reschedule_pe_activities(
    models: &[ActivityModel],
    states: &mut [ActivityState],
    context: &ScheduleContext,
) {
    let mut finish_by_node_idx = HashMap::new();
    let mut pe_ready_at = HashMap::new();
    for node_idx in &context.topological_order {
        let activity_idx = *context.activity_idx_by_node_idx.get(node_idx).unwrap();
        let model = &models[activity_idx];
        let state = &states[activity_idx];
        let deps_ready_at = model
            .node
            .analysis
            .predecessor_compute_node_indices
            .iter()
            .map(|pred| finish_by_node_idx.get(pred).copied().unwrap_or(0.0))
            .fold(0.0, f64::max);
        let (pe_name, prev_on_pe) = context.previous_on_pe.get(node_idx).unwrap();
        let lane_ready_at = prev_on_pe
            .as_ref()
            .and_then(|prev| finish_by_node_idx.get(prev).copied())
            .unwrap_or_else(|| pe_ready_at.get(pe_name).copied().unwrap_or(0.0));
        let start_ticks = deps_ready_at.max(lane_ready_at);
        let adjusted_memory_ticks = state.adjusted_memory_ticks_by_memory.values().sum::<f64>();
        let duration_ticks = model.node.compute_ticks.max(adjusted_memory_ticks);

        let state = &mut states[activity_idx];
        state.start_ticks = start_ticks;
        state.end_ticks = start_ticks + duration_ticks;
        finish_by_node_idx.insert(*node_idx, state.end_ticks);
        pe_ready_at.insert(pe_name.clone(), state.end_ticks);
    }
}

#[derive(Clone, Copy)]
enum MemoryEventKind {
    End,
    Start,
}

#[derive(Clone, Copy)]
struct MemoryEvent {
    time_ticks: f64,
    node_idx: usize,
    requested_fraction: f64,
    memory_fraction: f64,
    kind: MemoryEventKind,
}

fn build_memory_events(memory_name: &str, activities: &[ActivityView<'_>]) -> Vec<MemoryEvent> {
    let mut events = Vec::new();

    debug!(
        "build_memory_events: memory='{}' activities={}",
        memory_name,
        activities.len()
    );

    for activity in activities {
        let duration_ticks = activity.state.end_ticks - activity.state.start_ticks;
        let base_ticks = activity.model.base_memory_ticks(memory_name);
        let memory_ticks_sum = activity
            .model
            .base_memory_ticks_by_memory
            .values()
            .sum::<f64>();
        if duration_ticks <= 0.0 || base_ticks <= 0.0 {
            debug!(
                "build_memory_events: skipping node='{}' (node_idx={}) memory='{}' duration_ticks={:.3} base_ticks={:.3}",
                activity.model.node.analysis.id,
                activity.model.node.analysis.node_idx,
                memory_name,
                duration_ticks,
                base_ticks
            );
            continue;
        }

        let requested_fraction = base_ticks / duration_ticks;
        let memory_fraction = base_ticks / memory_ticks_sum;
        debug!(
            "build_memory_events: node='{}' (node_idx={}) memory='{}' start={:.3} end={:.3} duration={:.3} base_ticks={:.3} requested={:.3}, memory fraction={:.3}",
            activity.model.node.analysis.id,
            activity.model.node.analysis.node_idx,
            memory_name,
            activity.state.start_ticks,
            activity.state.end_ticks,
            duration_ticks,
            base_ticks,
            requested_fraction,
            memory_fraction
        );
        events.push(MemoryEvent {
            time_ticks: activity.state.start_ticks,
            node_idx: activity.model.node.analysis.node_idx,
            requested_fraction,
            memory_fraction,
            kind: MemoryEventKind::Start,
        });
        events.push(MemoryEvent {
            time_ticks: activity.state.end_ticks,
            node_idx: activity.model.node.analysis.node_idx,
            requested_fraction,
            memory_fraction,
            kind: MemoryEventKind::End,
        });
    }

    events.sort_by(|a, b| {
        a.time_ticks
            .partial_cmp(&b.time_ticks)
            .unwrap()
            .then_with(|| match (a.kind, b.kind) {
                (MemoryEventKind::End, MemoryEventKind::Start) => std::cmp::Ordering::Less,
                (MemoryEventKind::Start, MemoryEventKind::End) => std::cmp::Ordering::Greater,
                _ => a.node_idx.cmp(&b.node_idx),
            })
    });

    events
}

fn process_memory_events(memory_name: &str, events: &[MemoryEvent]) -> MemoryContentionAnalysis {
    let mut analysis = MemoryContentionAnalysis::default();
    let mut active: HashMap<usize, (f64, f64)> = HashMap::new();
    let mut event_idx = 0;

    debug!(
        "process_memory_events: memory='{}' events={}",
        memory_name,
        events.len()
    );

    while event_idx < events.len() {
        let time_ticks = events[event_idx].time_ticks;
        debug!("process_memory_events: memory='{memory_name}', processing time={time_ticks:.3}",);
        while event_idx < events.len()
            && events[event_idx].time_ticks == time_ticks
            && matches!(events[event_idx].kind, MemoryEventKind::End)
        {
            debug!(
                "process_memory_events: memory='{memory_name}', end node_idx={}",
                events[event_idx].node_idx,
            );
            active.remove(&events[event_idx].node_idx);
            event_idx += 1;
        }
        while event_idx < events.len()
            && events[event_idx].time_ticks == time_ticks
            && matches!(events[event_idx].kind, MemoryEventKind::Start)
        {
            debug!(
                "process_memory_events: memory='{memory_name}' start node_idx={}",
                events[event_idx].node_idx,
            );
            active.insert(
                events[event_idx].node_idx,
                (
                    events[event_idx].requested_fraction,
                    events[event_idx].memory_fraction,
                ),
            );
            event_idx += 1;
        }
        if event_idx >= events.len() {
            debug!("process_memory_events: memory='{memory_name}' reached final event boundary");
            break;
        }

        let next_time_ticks = events[event_idx].time_ticks;
        if next_time_ticks <= time_ticks || active.is_empty() {
            debug!(
                "process_memory_events: memory='{memory_name}' skipping window start={time_ticks:.3} end={next_time_ticks:.3} active_nodes={}",
                active.len()
            );
            continue;
        }

        let duration_ticks = next_time_ticks - time_ticks;
        let requested_fraction = active
            .values()
            .map(|(activity_fraction, _)| activity_fraction)
            .sum::<f64>();
        let slowdown = requested_fraction.max(1.0);
        let active_node_indices = active.keys().copied().collect::<Vec<_>>();
        debug!(
            "process_memory_events: memory='{memory_name}', window start={time_ticks:.3}, end={next_time_ticks:.3}, duration={duration_ticks:.3}, requested={requested_fraction:.3}, slowdown={slowdown:.3}, active={active_node_indices:?}",
        );
        for (node_idx, (_, memory_fraction)) in &active {
            let adjusted_ticks = memory_fraction * duration_ticks * slowdown;
            debug!(
                "process_memory_events: memory='{memory_name}', node_idx={node_idx}, memory fraction={memory_fraction:.3}, adjusted_ticks={adjusted_ticks:.3}",
            );
            *analysis
                .adjusted_ticks_by_node_idx
                .entry(*node_idx)
                .or_insert(0.0) += adjusted_ticks;
        }
        analysis.windows.push(MemoryContentionWindow {
            start_ticks: time_ticks,
            end_ticks: next_time_ticks,
            requested_fraction,
            active_node_indices,
        });
    }

    debug!(
        "process_memory_events: memory='{memory_name}', produced_windows={}, adjusted_nodes={}",
        analysis.windows.len(),
        analysis.adjusted_ticks_by_node_idx.len()
    );

    analysis
}

fn analyze_memory_contention(
    models: &[ActivityModel],
    states: &[ActivityState],
) -> BTreeMap<String, MemoryContentionAnalysis> {
    let mut analyses = BTreeMap::new();
    let mut activities_by_memory: BTreeMap<String, Vec<ActivityView<'_>>> = BTreeMap::new();

    for (model, state) in models.iter().zip(states.iter()) {
        for memory_name in model.base_memory_ticks_by_memory.keys() {
            activities_by_memory
                .entry(memory_name.clone())
                .or_default()
                .push(ActivityView { model, state });
        }
    }

    for (memory_name, memory_activities) in activities_by_memory {
        let events = build_memory_events(&memory_name, &memory_activities);
        analyses.insert(
            memory_name.clone(),
            process_memory_events(&memory_name, &events),
        );
    }

    analyses
}

fn apply_memory_contention_from_analysis(
    models: &[ActivityModel],
    states: &mut [ActivityState],
    analyses: &BTreeMap<String, MemoryContentionAnalysis>,
) {
    for (model, state) in models.iter().zip(states.iter_mut()) {
        let mut adjusted = BTreeMap::new();
        for memory_name in model.base_memory_ticks_by_memory.keys() {
            let base_ticks = model.base_memory_ticks(memory_name);
            let previous_ticks = state.adjusted_memory_ticks(memory_name);
            let adjusted_ticks = analyses
                .get(memory_name)
                .and_then(|analysis| {
                    analysis
                        .adjusted_ticks_by_node_idx
                        .get(&model.node.analysis.node_idx)
                        .copied()
                })
                .unwrap_or(base_ticks);
            let adjusted_ticks = adjusted_ticks.max(base_ticks);
            debug!(
                "apply_memory_contention_from_analysis: node='{}', memory='{memory_name}', base={base_ticks:.3}, previous={previous_ticks:.3}, adjusted={adjusted_ticks:.3}",
                model.node.analysis.id,
            );
            adjusted.insert(memory_name.clone(), adjusted_ticks);
        }
        state.adjusted_memory_ticks_by_memory = adjusted;
    }
}

fn build_pe_activities(models: &[ActivityModel], states: &[ActivityState]) -> Vec<PeActivity> {
    models
        .iter()
        .zip(states.iter())
        .map(|(model, state)| PeActivity {
            node: model.node.clone(),
            pe_name: model.pe_name.clone(),
            base_memory_ticks_by_memory: model.base_memory_ticks_by_memory.clone(),
            adjusted_memory_ticks_by_memory: state.adjusted_memory_ticks_by_memory.clone(),
            start_ticks: state.start_ticks,
            end_ticks: state.end_ticks,
        })
        .collect()
}

pub fn schedule_pe_activities(
    node_rooflines: &[ComputeNodeRoofline],
    bandwidth_graph: &BandwidthGraph,
    widest_path_cache: &mut WidestPathCache,
) -> Result<ScheduledActivities, Box<dyn std::error::Error>> {
    let models = build_activity_models(node_rooflines, bandwidth_graph, widest_path_cache)?;
    let context = build_schedule_context(node_rooflines);
    let mut states = initialize_activity_states(&models);

    reschedule_pe_activities(&models, &mut states, &context);
    let mut analyses = analyze_memory_contention(&models, &states);
    for _ in 0..NUM_SCHEDULE_ATTEMPTS {
        let previous_timings = states
            .iter()
            .map(|state| (state.start_ticks, state.end_ticks))
            .collect::<Vec<_>>();
        apply_memory_contention_from_analysis(&models, &mut states, &analyses);
        reschedule_pe_activities(&models, &mut states, &context);
        analyses = analyze_memory_contention(&models, &states);

        let stable = states
            .iter()
            .zip(previous_timings)
            .all(|(next, (prev_start, prev_end))| {
                (next.start_ticks - prev_start).abs() < 1e-6
                    && (next.end_ticks - prev_end).abs() < 1e-6
            });
        if stable {
            break;
        }
    }

    // Recompute contention from the stabilized schedule so the returned
    // memory windows match the final per-PE activity timing we report.
    analyses = analyze_memory_contention(&models, &states);
    apply_memory_contention_from_analysis(&models, &mut states, &analyses);

    let mut activities = build_pe_activities(&models, &states);
    activities.sort_by(|a, b| {
        a.start_ticks
            .partial_cmp(&b.start_ticks)
            .unwrap()
            .then_with(|| a.node.analysis.node_idx.cmp(&b.node.analysis.node_idx))
    });
    Ok(ScheduledActivities {
        activities,
        memory_analyses: analyses,
    })
}

#[must_use]
pub fn critical_path_analysis(node_rooflines: &[ComputeNodeRoofline]) -> CriticalPathAnalysis {
    let node_by_idx = node_rooflines
        .iter()
        .map(|node| (node.analysis.node_idx, node))
        .collect::<HashMap<_, _>>();
    let mut indegree = HashMap::new();
    let mut successors: HashMap<usize, Vec<usize>> = HashMap::new();

    for node in node_rooflines {
        indegree.insert(
            node.analysis.node_idx,
            node.analysis.predecessor_compute_node_indices.len(),
        );
        for predecessor in &node.analysis.predecessor_compute_node_indices {
            successors
                .entry(*predecessor)
                .or_default()
                .push(node.analysis.node_idx);
        }
    }

    let mut ready = BTreeSet::new();
    for (node_idx, degree) in &indegree {
        if *degree == 0 {
            ready.insert(*node_idx);
        }
    }

    let mut best_finish: HashMap<usize, f64> = HashMap::new();
    let mut best_predecessor: HashMap<usize, Option<usize>> = HashMap::new();
    while let Some(node_idx) = ready.pop_first() {
        let node = node_by_idx.get(&node_idx).unwrap();
        let mut start = 0.0;
        let mut predecessor_on_path = None;
        for predecessor in &node.analysis.predecessor_compute_node_indices {
            let finish = best_finish.get(predecessor).copied().unwrap_or(0.0);
            if finish > start {
                start = finish;
                predecessor_on_path = Some(*predecessor);
            }
        }
        let finish = start + node.roofline_ticks;
        best_finish.insert(node_idx, finish);
        best_predecessor.insert(node_idx, predecessor_on_path);

        if let Some(next_nodes) = successors.get(&node_idx) {
            for next in next_nodes {
                let degree = indegree.get_mut(next).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(*next);
                }
            }
        }
    }

    let Some((end_node_idx, total_ticks)) = best_finish
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(node_idx, ticks)| (*node_idx, *ticks))
    else {
        return CriticalPathAnalysis {
            total_ticks: 0.0,
            node_indices: Vec::new(),
        };
    };

    let mut node_indices = Vec::new();
    let mut current = Some(end_node_idx);
    while let Some(node_idx) = current {
        node_indices.push(node_idx);
        current = best_predecessor.get(&node_idx).copied().flatten();
    }
    node_indices.reverse();

    CriticalPathAnalysis {
        total_ticks,
        node_indices,
    }
}
