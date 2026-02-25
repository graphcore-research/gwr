// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A simple front-end for running a `Timetable` on a `Platform`
//!
//! For example, run using:
//!   cargo run --bin gwr-timetable -- --platform
//! gwr-platform/examples/platform.yaml --graph
//! gwr-timetable/examples/graph.yaml --stdout --stdout-level debug

use std::path::Path;
use std::rc::Rc;

use anyhow::Result;
use clap::Parser;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::types::Graph;
use gwr_track::Track;
use gwr_track::builder::{MonitorsConfig, TrackerConfig, TrackersConfig, setup_trackers};
use indicatif::ProgressBar;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Application to load and validate a timetable against the schema")]
struct Cli {
    /// Enable logging to the console.
    #[arg(long, default_value = "false")]
    stdout: bool,

    /// Level of log message to display.
    #[arg(long, default_value = "Info")]
    stdout_level: log::Level,

    /// Set a regular expression for which entites should have logging level set
    /// to `--stdout-level`. Others will have level set to `Error`.
    #[arg(long, default_value = "")]
    stdout_filter_regex: String,

    /// Enable logging to binary file used by `gwr-spotter`.
    #[arg(long, default_value = "false")]
    binary: bool,

    /// Level of binary trace events to record.
    #[arg(long, default_value = "Trace")]
    binary_level: log::Level,

    /// Set a regular expression for which entites should have binary output
    /// level set to `--binary-level`. Others will have level set to
    /// `Error`.
    #[arg(long, default_value = "")]
    binary_filter_regex: String,

    /// The filename binary trace output is written to.
    #[arg(long, default_value = "trace.bin")]
    binary_file: String,

    /// Enable logging to Perfetto file used by `gwr-spotter`.
    #[arg(long, default_value = "false")]
    perfetto: bool,

    /// Level of Perfetto trace events to record.
    #[arg(long, default_value = "Trace")]
    perfetto_level: log::Level,

    /// Set a regular expression for which entites should have Perfetto output
    /// level set to `--perfetto-level`. Others will have level set to
    /// `Error`.
    #[arg(long, default_value = "")]
    perfetto_filter_regex: String,

    /// The filename Perfetto trace output is written to.
    #[arg(long, default_value = "trace.pftrace")]
    perfetto_file: String,

    /// Enable monitoring at the specified number of clock ticks.
    #[clap(long)]
    monitor_window_ticks: Option<u64>,

    /// Set a regular expression for which ports should have monitors
    /// enabled.
    #[arg(long, default_value = "")]
    monitor_filter_regex: String,

    /// Show a progress bar for the received frame count (updated at the rate
    /// defined by `progress_ticks`).
    #[arg(long)]
    progress: bool,

    /// Number of ticks between updates to the progress bar. Only used when
    /// `progress` is enabled.
    #[arg(long, default_value = "1000")]
    progress_ticks: usize,

    /// Graph file
    #[arg(long, default_value = "graph.yaml")]
    graph: String,

    /// Platform file
    #[arg(long, default_value = "platform.yaml")]
    platform: String,
}

fn start_frame_dump(
    spawner: &Spawner,
    clock: Clock,
    progress_ticks: usize,
    total_expected_tasks: usize,
    timetable: Rc<Timetable>,
    progress_bar: ProgressBar,
) {
    spawner.spawn(async move {
        let mut seen_frames = 0;
        loop {
            // Use the `background` wait to indicate that the simulation can end if this is
            // the only task still active.
            clock.wait_ticks_or_exit(progress_ticks as u64).await;
            let num_completed_tasks: usize = timetable.num_graph_nodes_completed();
            progress_bar.inc((num_completed_tasks - seen_frames) as u64);
            seen_frames = num_completed_tasks;
            if num_completed_tasks == total_expected_tasks {
                break;
            }
        }
        Ok(())
    });
}

fn setup_all_trackers(args: &Cli) -> Rc<dyn Track> {
    let config = TrackersConfig {
        stdout: TrackerConfig {
            enable: args.stdout,
            level: args.stdout_level,
            filter_regex: &args.stdout_filter_regex,
            file: None,
        },
        binary: TrackerConfig {
            enable: args.binary,
            level: args.binary_level,
            filter_regex: &args.binary_filter_regex,
            file: Some(&args.binary_file),
        },
        perfetto: TrackerConfig {
            enable: args.perfetto,
            level: args.perfetto_level,
            filter_regex: &args.perfetto_filter_regex,
            file: Some(&args.perfetto_file),
        },
        monitors: MonitorsConfig {
            enable: args.monitor_window_ticks.is_some(),
            window_size_ticks: args.monitor_window_ticks.unwrap_or(0),
            filter_regex: &args.monitor_filter_regex,
        },
    };
    setup_trackers(&config).unwrap()
}

fn main() -> Result<()> {
    let args = Cli::parse();
    let tracker = setup_all_trackers(&args);

    let graph_path = Path::new(&args.graph);

    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_file(
        &engine,
        &clock,
        Path::new(&args.platform),
    )?);

    println!("Loaded platform:");
    println!("{platform}");

    let graph = Graph::from_file(graph_path)?;
    let num_nodes = graph.nodes.len();
    let num_edges = graph.edges.len();

    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform)?);
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);

    println!("Loaded graph with {num_nodes} nodes, {num_edges} edges.");

    let mut progress_bar = None;
    if args.progress {
        let total_expected_tasks = timetable.total_tasks();
        progress_bar = Some(ProgressBar::new(total_expected_tasks as u64));
        let spawner = engine.spawner();
        start_frame_dump(
            &spawner,
            clock.clone(),
            args.progress_ticks,
            total_expected_tasks,
            timetable.clone(),
            progress_bar.clone().unwrap(),
        );
    }

    engine.run()?;

    if let Some(progress_bar) = progress_bar {
        progress_bar.finish();
    }

    println!("Ran simulation. Time now {}ns", clock.time_now_ns());

    timetable.check_tasks_complete()?;

    Ok(())
}
