// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, HashMap};

use gwr_engine::time::clock::Clock;
use gwr_platform::Platform;

use crate::analysis::memory::MemoryContentionAnalysis;
use crate::analysis::pe::{ComputeNodeRoofline, PeActivity, PeRooflineSummary};
use crate::analysis::roofline::RooflineReport;
use crate::analysis::{
    bytes_per_tick_to_gb_per_s, compute_adjusted_value_and_rate, format_bytes, ticks_to_ns,
};
use crate::{TimetableStats, format_machine_ops};

#[derive(Clone, Debug)]
pub struct ReportOptions {
    pub top: usize,
    pub node_rankings: bool,
    pub pe_summary: bool,
    pub activity_report: bool,
    pub memory_report: bool,
    pub scheduled_runtime: bool,
    pub critical_path_nodes: bool,
    pub show_deps: bool,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            top: 10,
            node_rankings: false,
            pe_summary: false,
            activity_report: false,
            memory_report: false,
            scheduled_runtime: true,
            critical_path_nodes: false,
            show_deps: false,
        }
    }
}

impl ReportOptions {
    #[must_use]
    pub fn full(top: usize, show_deps: bool) -> Self {
        Self {
            top,
            node_rankings: true,
            pe_summary: true,
            activity_report: true,
            memory_report: true,
            scheduled_runtime: true,
            critical_path_nodes: true,
            show_deps,
        }
    }

    #[must_use]
    pub fn needs_scheduled_activities(&self) -> bool {
        self.activity_report || self.memory_report || self.scheduled_runtime
    }
}

fn print_ranked_nodes<F>(
    clock: &Clock,
    title: &str,
    nodes: &[ComputeNodeRoofline],
    top: usize,
    mut sort_key: F,
    reverse: bool,
) where
    F: FnMut(&ComputeNodeRoofline) -> f64,
{
    let mut ranked = nodes.to_vec();
    ranked.sort_by(|a, b| sort_key(a).partial_cmp(&sort_key(b)).unwrap());
    if reverse {
        ranked.reverse();
    }

    println!("\n{title}:");
    for node in ranked.into_iter().take(top) {
        let pe_name = node.analysis.pe_name.as_deref().unwrap_or("<none>");
        println!(
            "  {} on {}: flops={} [{}] bytes={} ({:.2}) flops/byte={:.3} compute={:.2} ns memory={:.2} ns roofline={:.2} ns",
            node.analysis.id,
            pe_name,
            node.analysis.flops,
            format_machine_ops(&node.analysis.machine_ops),
            node.analysis.total_bytes(),
            format_bytes(node.analysis.total_bytes()),
            node.analysis.flops_per_byte(),
            ticks_to_ns(clock, node.compute_ticks),
            ticks_to_ns(clock, node.memory_ticks),
            ticks_to_ns(clock, node.roofline_ticks)
        );
    }
}

pub fn print_timetable_summary(stats: &TimetableStats) {
    println!(
        "Timetable has {} compute nodes, {} tensor nodes, {} memory nodes.",
        stats.num_compute_nodes, stats.num_tensor_nodes, stats.num_memory_nodes
    );
    println!(
        "Timetable traffic totals: loads {} ({:.2}), stores {} ({:.2}), machine ops {} [{}].",
        stats.total_load_bytes,
        format_bytes(stats.total_load_bytes),
        stats.total_store_bytes,
        format_bytes(stats.total_store_bytes),
        stats.total_machine_ops,
        format_machine_ops(&stats.machine_ops)
    );
}

pub fn print_node_rankings(clock: &Clock, top: usize, node_rooflines: &[ComputeNodeRoofline]) {
    print_ranked_nodes(
        clock,
        "Most Compute-Heavy Nodes",
        node_rooflines,
        top,
        |node| node.analysis.flops as f64,
        true,
    );
    print_ranked_nodes(
        clock,
        "Nodes Fetching The Most Data",
        node_rooflines,
        top,
        |node| node.analysis.total_bytes() as f64,
        true,
    );
    print_ranked_nodes(
        clock,
        "Highest FLOPs Per Byte",
        node_rooflines,
        top,
        |node| node.analysis.flops_per_byte(),
        true,
    );
    print_ranked_nodes(
        clock,
        "Lowest FLOPs Per Byte",
        node_rooflines,
        top,
        |node| node.analysis.flops_per_byte(),
        false,
    );
}

pub fn print_pe_roofline_summary(clock: &Clock, pe_summaries: &[PeRooflineSummary]) {
    println!("\nPer-PE Roofline Summary:");
    for summary in pe_summaries {
        let memory_breakdown = if summary.bytes_by_memory.is_empty() {
            "none".to_string()
        } else {
            summary
                .bytes_by_memory
                .iter()
                .map(|(memory, bytes)| format!("{memory}: {:.2}", format_bytes(*bytes)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "  {}: nodes={} flops={} bytes={} ({:.2}) compute={:.2} ns memory={:.2} ns roofline={:.2} ns [{}]",
            summary.pe_name,
            summary.compute_nodes,
            summary.total_flops,
            summary.total_bytes,
            format_bytes(summary.total_bytes),
            ticks_to_ns(clock, summary.compute_ticks),
            ticks_to_ns(clock, summary.memory_ticks),
            ticks_to_ns(clock, summary.roofline_ticks),
            memory_breakdown
        );
    }
}

fn print_activity(
    clock: &Clock,
    activity: &PeActivity,
    previous_end_ticks: f64,
    show_deps: bool,
) -> f64 {
    if activity.start_ticks > previous_end_ticks {
        println!(
            "    {:.2}ns ({:.2}ns -> {:.2}ns) | IDLE",
            ticks_to_ns(clock, activity.start_ticks - previous_end_ticks),
            ticks_to_ns(clock, previous_end_ticks),
            ticks_to_ns(clock, activity.start_ticks),
        );
    }
    let base_memory_ticks = activity.base_memory_ticks_by_memory.values().sum::<f64>();
    let adjusted_memory_ticks = activity
        .adjusted_memory_ticks_by_memory
        .values()
        .sum::<f64>();
    let memory_breakdown = if activity.adjusted_memory_ticks_by_memory.is_empty() {
        "none".to_string()
    } else {
        activity
            .adjusted_memory_ticks_by_memory
            .iter()
            .map(|(memory_name, ticks)| {
                let base_ticks = activity.base_memory_ticks(memory_name);
                format!(
                    "{memory_name}: {:.2}ns->{:.2}ns",
                    ticks_to_ns(clock, base_ticks),
                    ticks_to_ns(clock, *ticks)
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let start_ns = ticks_to_ns(clock, activity.start_ticks);
    let end_ns = ticks_to_ns(clock, activity.end_ticks);
    let duration_ns = end_ns - start_ns;
    let deps_suffix = if show_deps {
        activity.deps_string()
    } else {
        String::new()
    };

    println!(
        "    {:.2}ns ({:.2}ns -> {:.2}ns) | {} compute={:.2}ns memory={:.2}ns->{:.2}ns bytes={} ({:.2}) flops={} [{}] mem=[{}]{}",
        duration_ns,
        start_ns,
        end_ns,
        activity.node.analysis.id,
        ticks_to_ns(clock, activity.node.compute_ticks),
        ticks_to_ns(clock, base_memory_ticks),
        ticks_to_ns(clock, adjusted_memory_ticks),
        activity.node.analysis.total_bytes(),
        format_bytes(activity.node.analysis.total_bytes()),
        activity.node.analysis.flops,
        format_machine_ops(&activity.node.analysis.machine_ops),
        memory_breakdown,
        deps_suffix
    );

    activity.end_ticks
}

pub fn print_pe_activity_report(clock: &Clock, activities: &[PeActivity], show_deps: bool) {
    let mut activities_by_pe: BTreeMap<String, Vec<&PeActivity>> = BTreeMap::new();
    for activity in activities {
        activities_by_pe
            .entry(activity.pe_name.clone())
            .or_default()
            .push(activity);
    }

    println!("\nPer-PE Activity Report:");
    for (pe_name, pe_activities) in activities_by_pe {
        println!("  {pe_name}:");
        let mut previous_end_ticks = 0.0;
        for activity in pe_activities {
            previous_end_ticks = print_activity(clock, activity, previous_end_ticks, show_deps);
        }
    }
}

fn print_contention_window(
    clock: &Clock,
    memory_name: &str,
    window: &crate::analysis::memory::MemoryContentionWindow,
    activity_by_node_idx: &HashMap<usize, &PeActivity>,
) {
    let oversubscribed_fraction = (window.requested_fraction - 1.0).max(0.0);
    let active_nodes = window
        .active_node_indices
        .iter()
        .filter_map(|node_idx| {
            let activity = activity_by_node_idx.get(node_idx)?;
            activity.memory_summary_string(memory_name)
        })
        .collect::<Vec<_>>()
        .join(", ");

    println!(
        "    {:.2}ns -> {:.2}ns | requested={:.0}% oversubscribed={:.0}% active=[{}]",
        ticks_to_ns(clock, window.start_ticks),
        ticks_to_ns(clock, window.end_ticks),
        window.requested_fraction * 100.0,
        oversubscribed_fraction * 100.0,
        active_nodes
    );
}

fn print_memory_analysis(
    clock: &Clock,
    platform: &Platform,
    memory_name: &str,
    analysis: &MemoryContentionAnalysis,
    activities: &[PeActivity],
    activity_by_node_idx: &HashMap<usize, &PeActivity>,
) {
    println!("  {memory_name}:");
    if analysis.windows.is_empty() {
        println!("    no scheduled activity");
        return;
    }

    let memory_bandwidth = platform.memory(memory_name).unwrap().bw_bytes_per_cycle() as f64;
    let total_bytes = activities
        .iter()
        .map(|activity| activity.node.memory_bytes(memory_name))
        .sum::<usize>();
    let achieved_bytes_per_tick = analysis.achieved_bytes_per_tick(memory_bandwidth);
    let average_oversubscription = analysis.average_oversubscription();

    println!(
        "    summary: bytes={total_bytes} ({:.2}) achieved_bw={achieved_bytes_per_tick:.2}/{memory_bandwidth:.2} bytes/tick ({:.2} GB/s) avg_oversubscription={:.1}%",
        format_bytes(total_bytes),
        bytes_per_tick_to_gb_per_s(clock, achieved_bytes_per_tick),
        average_oversubscription * 100.0
    );

    for window in &analysis.windows {
        print_contention_window(clock, memory_name, window, activity_by_node_idx);
    }
}

pub fn print_memory_contention_report(
    clock: &Clock,
    platform: &Platform,
    activities: &[PeActivity],
    memory_analyses: &BTreeMap<String, MemoryContentionAnalysis>,
) {
    let activity_by_node_idx = activities
        .iter()
        .map(|activity| (activity.node.analysis.node_idx, activity))
        .collect::<HashMap<_, _>>();
    println!("\nPer-Memory Oversubscription Report:");
    for (memory_name, analysis) in memory_analyses {
        print_memory_analysis(
            clock,
            platform,
            memory_name,
            analysis,
            activities,
            &activity_by_node_idx,
        );
    }
}

pub fn print_runtime_estimate(clock: &Clock, report: &RooflineReport, options: &ReportOptions) {
    let node_by_idx = report
        .node_rooflines
        .iter()
        .map(|node| (node.analysis.node_idx, node))
        .collect::<HashMap<_, _>>();
    let total_flops = report.timetable_stats.total_machine_ops as f64;
    let best_case_flops = compute_adjusted_value_and_rate(
        total_flops,
        ticks_to_ns(clock, report.estimated_best_case_ticks) / 1_000_000_000.0,
        "FLOP",
    );

    println!("\nBest-Case Runtime Estimate:");
    println!(
        "  Compute-node critical path: {:.2} ns",
        ticks_to_ns(clock, report.critical_path.total_ticks)
    );
    println!(
        "  Max per-PE roofline lower bound: {:.2} ns",
        ticks_to_ns(clock, report.pe_lower_bound_ticks)
    );
    println!(
        "  Estimated overall best-case runtime: {:.2} ns, flops: {}",
        ticks_to_ns(clock, report.estimated_best_case_ticks),
        best_case_flops
    );

    if options.scheduled_runtime {
        if let Some(scheduled_runtime_ticks) = report.scheduled_runtime_ticks {
            let scheduled_flops = compute_adjusted_value_and_rate(
                total_flops,
                ticks_to_ns(clock, scheduled_runtime_ticks) / 1_000_000_000.0,
                "FLOP",
            );
            println!(
                "  Post-memory scheduled runtime: {:.2} ns, flops: {}",
                ticks_to_ns(clock, scheduled_runtime_ticks),
                scheduled_flops
            );
            println!(
                "  Memory-analysis impact vs best-case: {:+.2} ns ({:.2} times slower)",
                ticks_to_ns(
                    clock,
                    scheduled_runtime_ticks - report.estimated_best_case_ticks
                ),
                if report.estimated_best_case_ticks > 0.0 {
                    scheduled_runtime_ticks / report.estimated_best_case_ticks
                } else {
                    1.0
                }
            );
        } else {
            println!("  Post-memory scheduled runtime: not computed");
        }
    }

    if options.critical_path_nodes && !report.critical_path.node_indices.is_empty() {
        println!("  Worst-case path nodes:");
        for node_idx in &report.critical_path.node_indices {
            let node = node_by_idx.get(node_idx).unwrap();
            let pe_name = node.analysis.pe_name.as_deref().unwrap_or("<none>");
            println!(
                "    {} on {}: roofline={:.2} ns compute={:.2} ns memory={:.2} ns bytes={} ({:.2}) flops={} [{}]",
                node.analysis.id,
                pe_name,
                ticks_to_ns(clock, node.roofline_ticks),
                ticks_to_ns(clock, node.compute_ticks),
                ticks_to_ns(clock, node.memory_ticks),
                node.analysis.total_bytes(),
                format_bytes(node.analysis.total_bytes()),
                node.analysis.flops,
                format_machine_ops(&node.analysis.machine_ops),
            );
        }
    }
}

pub fn print_roofline_report(
    clock: &Clock,
    platform: &Platform,
    report: &RooflineReport,
    options: &ReportOptions,
) {
    print_timetable_summary(&report.timetable_stats);
    if options.node_rankings {
        print_node_rankings(clock, options.top, &report.node_rooflines);
    }
    if options.pe_summary {
        print_pe_roofline_summary(clock, &report.pe_summaries);
    }
    if let Some(scheduled_activities) = &report.scheduled_activities {
        if options.activity_report {
            print_pe_activity_report(clock, &scheduled_activities.activities, options.show_deps);
        }
        if options.memory_report {
            print_memory_contention_report(
                clock,
                platform,
                &scheduled_activities.activities,
                &scheduled_activities.memory_analyses,
            );
        }
    }
    print_runtime_estimate(clock, report, options);
}
