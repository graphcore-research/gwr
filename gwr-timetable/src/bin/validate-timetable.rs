// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

// Known limitations: Tensors are assumed to not span memories.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

use clap::Parser;
use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::analysis::memory::{
    BandwidthGraph, MemoryContentionAnalysis, WidestPathCache, resource_bytes_per_cycle,
};
use gwr_timetable::analysis::pe::{
    ComputeNodeRoofline, PeActivity, PeRooflineSummary, aggregate_pe_rooflines,
    compute_node_rooflines, critical_path_analysis, schedule_pe_activities,
};
use gwr_timetable::analysis::{format_bytes, ticks_to_ns};
use gwr_timetable::timetable_file::TimetableFile;
use gwr_timetable::{ComputeNodeAnalysis, Timetable, format_machine_ops};
use log::{LevelFilter, Metadata, Record};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct SimpleLogger;

static LOGGER: SimpleLogger = SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{}", record.args());
        }
    }

    fn flush(&self) {}
}

fn init_logging(debug_enabled: bool) {
    let level = if debug_enabled {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);
}

#[derive(Debug, Parser)]
#[command(about = "Load, validate, and analyse a timetable against a platform")]
struct Args {
    /// Timetable YAML file to validate and analyse.
    #[arg(long, default_value = "timetable.yaml")]
    timetable: PathBuf,

    /// Platform YAML file to validate against.
    #[arg(long, default_value = "platform.yaml")]
    platform: PathBuf,

    /// Print the constructed platform after validation.
    #[arg(long, default_value_t = false)]
    print_platform: bool,

    /// Number of nodes to show in each ranked report.
    #[arg(long, default_value_t = 10)]
    top: usize,

    /// Enable debug output for internal analysis steps.
    #[arg(long, default_value_t = false)]
    debug: bool,

    /// Include explicit dependency lists in the per-PE activity report.
    #[arg(long, default_value_t = false)]
    show_deps: bool,
}

struct AnalysisContext {
    clock: Clock,
    platform: Rc<Platform>,
    timetable: Timetable,
    bandwidth_graph: BandwidthGraph,
    widest_path_cache: WidestPathCache,
}

fn parse_platform_config(path: &PathBuf) -> Result<PlatformConfig> {
    let content = std::fs::read_to_string(path)?;
    let cfg = serde_yaml::from_str(&content)?;
    Ok(cfg)
}

fn build_analysis_context(args: &Args) -> Result<AnalysisContext> {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_file(&engine, &clock, &args.platform)?);
    let platform_cfg = parse_platform_config(&args.platform)?;

    let timetable_file = TimetableFile::from_file(&args.timetable)?;
    let timetable = Timetable::new(engine.top(), timetable_file, &platform)?;

    let bytes_per_cycle = resource_bytes_per_cycle(&platform)?;
    let bandwidth_graph = BandwidthGraph::build(&platform_cfg, &bytes_per_cycle)?;

    Ok(AnalysisContext {
        clock,
        platform,
        timetable,
        bandwidth_graph,
        widest_path_cache: WidestPathCache::default(),
    })
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

fn print_validation_summary(args: &Args, platform: &Platform) {
    println!(
        "Validated timetable '{}' against platform '{}'.",
        args.timetable.display(),
        args.platform.display()
    );
    println!(
        "Platform has {} PEs, {} caches, {} memories, and {} fabrics.",
        platform.num_pes(),
        platform.num_caches(),
        platform.num_memories(),
        platform.num_fabrics()
    );

    if args.print_platform {
        println!("{platform}");
    }
}

fn print_timetable_summary(timetable: &Timetable) -> Result<Vec<ComputeNodeAnalysis>> {
    let compute_nodes = timetable.compute_node_analyses()?;
    let timetable_stats = timetable.stats()?;
    println!(
        "Timetable has {} compute nodes, {} tensor nodes, {} memory nodes.",
        timetable_stats.num_compute_nodes,
        timetable_stats.num_tensor_nodes,
        timetable_stats.num_memory_nodes
    );
    println!(
        "Timetable traffic totals: loads {} ({:.2}), stores {} ({:.2}), machine ops {} [{}].",
        timetable_stats.total_load_bytes,
        format_bytes(timetable_stats.total_load_bytes),
        timetable_stats.total_store_bytes,
        format_bytes(timetable_stats.total_store_bytes),
        timetable_stats.total_machine_ops,
        format_machine_ops(&timetable_stats.machine_ops)
    );
    Ok(compute_nodes)
}

fn print_node_rankings(clock: &Clock, top: usize, node_rooflines: &[ComputeNodeRoofline]) {
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

fn print_pe_roofline_summary(clock: &Clock, pe_summaries: &[PeRooflineSummary]) {
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

fn print_pe_activity_report(clock: &Clock, activities: &[PeActivity], show_deps: bool) {
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
            previous_end_ticks = activity.print_report(clock, previous_end_ticks, show_deps);
        }
    }
}

fn print_memory_contention_report(
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
        analysis.print_report(
            clock,
            platform,
            memory_name,
            activities,
            &activity_by_node_idx,
        );
    }
}

fn print_runtime_estimate(
    clock: &Clock,
    node_rooflines: &[ComputeNodeRoofline],
    pe_summaries: &[PeRooflineSummary],
    activities: &[PeActivity],
) {
    let critical_path = critical_path_analysis(node_rooflines);
    let node_by_idx = node_rooflines
        .iter()
        .map(|node| (node.analysis.node_idx, node))
        .collect::<HashMap<_, _>>();
    let pe_lower_bound = pe_summaries
        .iter()
        .map(|summary| summary.roofline_ticks)
        .fold(0.0, f64::max);
    let estimated_best_case_runtime = critical_path.total_ticks.max(pe_lower_bound);
    let post_memory_runtime = activities
        .iter()
        .map(|activity| activity.end_ticks)
        .fold(0.0, f64::max);

    println!("\nBest-Case Runtime Estimate:");
    println!(
        "  Compute-node critical path: {:.2} ns",
        ticks_to_ns(clock, critical_path.total_ticks)
    );
    println!(
        "  Max per-PE roofline lower bound: {:.2} ns",
        ticks_to_ns(clock, pe_lower_bound)
    );
    println!(
        "  Estimated overall best-case runtime: {:.2} ns",
        ticks_to_ns(clock, estimated_best_case_runtime)
    );
    println!(
        "  Post-memory scheduled runtime: {:.2} ns",
        ticks_to_ns(clock, post_memory_runtime)
    );
    println!(
        "  Memory-analysis impact vs best-case: {:+.2} ns ({:.2} times slower)",
        ticks_to_ns(clock, post_memory_runtime - estimated_best_case_runtime),
        post_memory_runtime / estimated_best_case_runtime
    );

    if !critical_path.node_indices.is_empty() {
        println!("  Worst-case path nodes:");
        for node_idx in critical_path.node_indices {
            let node = node_by_idx.get(&node_idx).unwrap();
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

fn main() -> Result<()> {
    let args = Args::parse();
    init_logging(args.debug);

    let mut ctx = build_analysis_context(&args)?;

    print_validation_summary(&args, &ctx.platform);
    let compute_nodes = print_timetable_summary(&ctx.timetable)?;

    if compute_nodes.is_empty() {
        println!("\nNo compute nodes found.");
        return Ok(());
    }

    let node_rooflines = compute_node_rooflines(
        &ctx.platform,
        &compute_nodes,
        &ctx.bandwidth_graph,
        &mut ctx.widest_path_cache,
    )?;

    print_node_rankings(&ctx.clock, args.top, &node_rooflines);

    let pe_summaries = aggregate_pe_rooflines(
        &ctx.platform,
        &node_rooflines,
        &ctx.bandwidth_graph,
        &mut ctx.widest_path_cache,
    )?;
    let scheduled_activities = schedule_pe_activities(
        &node_rooflines,
        &ctx.bandwidth_graph,
        &mut ctx.widest_path_cache,
    )?;
    print_pe_roofline_summary(&ctx.clock, &pe_summaries);
    print_pe_activity_report(&ctx.clock, &scheduled_activities.activities, args.show_deps);
    print_memory_contention_report(
        &ctx.clock,
        &ctx.platform,
        &scheduled_activities.activities,
        &scheduled_activities.memory_analyses,
    );
    print_runtime_estimate(
        &ctx.clock,
        &node_rooflines,
        &pe_summaries,
        &scheduled_activities.activities,
    );

    Ok(())
}
