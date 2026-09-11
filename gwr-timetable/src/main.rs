// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A simple front-end for running a [Timetable] on a [Platform].

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gwr_config::multi_source_config;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::Track;
use gwr_track::builder::{TrackerArgs, TrackerArgsPartial, setup_trackers};
use indicatif::ProgressBar;

type AppResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Command-line arguments.
#[multi_source_config]
#[derive(PartialEq)]
#[command(about = "Run a timetable on a platform and optionally emit traces and summary stats")]
struct Cli {
    #[command(flatten)]
    #[serde(flatten)]
    tracker: TrackerArgs,

    /// Show a progress bar for the received frame count (updated at the rate
    /// defined by `progress_ticks`).
    #[arg(long, default_value_t = false)]
    progress: Option<bool>,

    /// Number of ticks between updates to the progress bar. Only used when
    /// `progress` is enabled.
    #[arg(long, default_value_t = 1000)]
    progress_ticks: Option<usize>,

    /// Timetable YAML file
    #[arg(long, default_value_t = PathBuf::from("timetable.yaml"))]
    timetable: Option<PathBuf>,

    /// Platform YAML file
    #[arg(long, default_value_t = PathBuf::from("platform.yaml"))]
    platform: Option<PathBuf>,

    /// Enable dumping of summary statistics
    #[arg(long, default_value_t = false)]
    dump_stats: Option<bool>,

    /// Write a Mermaid diagram of the timetable state to this file if execution
    /// fails.
    #[arg(long, default_value_t = PathBuf::from("error.mmd"))]
    error_mermaid: Option<PathBuf>,
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
        let mut seen_completed_tasks = 0;
        loop {
            // Use the `background` wait to indicate that the simulation can end
            // if this is the only task still active.
            clock.wait_ticks_or_exit(progress_ticks as u64).await;
            let num_completed_tasks: usize = timetable.num_graph_nodes_completed();
            progress_bar.inc((num_completed_tasks - seen_completed_tasks) as u64);
            seen_completed_tasks = num_completed_tasks;
            if num_completed_tasks == total_expected_tasks {
                break;
            }
        }
        Ok(())
    });
}

fn write_error_mermaid(timetable: &Timetable, path: &Path) {
    let mermaid = timetable.render_mermaid();
    if let Err(err) = fs::write(path, mermaid) {
        eprintln!(
            "Failed to write Mermaid timetable state to '{}': {err}",
            path.display()
        );
    } else {
        eprintln!("Wrote Mermaid timetable state to '{}'", path.display());
    }
}

fn main() -> AppResult<()> {
    let mut args = Cli::parse_all_sources();
    let dump_stats = args.dump_stats.unwrap();
    args.tracker
        .ensure_visiblity(dump_stats, "--dump-stats", log::Level::Info);

    let tracker: Rc<dyn Track> = setup_trackers(&args.tracker.trackers_config()).unwrap();
    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let platform_path = args.platform.unwrap();
    let platform = Rc::new(Platform::from_file(
        &engine,
        &clock,
        Path::new(&platform_path),
    )?);

    println!("Loaded platform:\n{platform}");

    let timetable_file = TimetableFile::from_file(&args.timetable.unwrap())?;
    let num_nodes = timetable_file.nodes.len();
    let num_edges = timetable_file.edges.len();
    let graph = timetable_file.into_graph()?;

    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform)?);
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);

    println!("Loaded timetable with {num_nodes} nodes, {num_edges} edges.");

    let mut progress_bar = None;
    if args.progress.unwrap() {
        let total_expected_tasks = timetable.total_tasks();
        progress_bar = Some(ProgressBar::new(total_expected_tasks as u64));
        let spawner = engine.spawner();
        start_frame_dump(
            &spawner,
            clock.clone(),
            args.progress_ticks.unwrap(),
            total_expected_tasks,
            timetable.clone(),
            progress_bar.clone().unwrap(),
        );
    }

    let run_result = engine.run();

    if let Some(progress_bar) = progress_bar {
        progress_bar.finish();
    }

    if let Err(err) = run_result {
        write_error_mermaid(&timetable, &args.error_mermaid.unwrap());
        return Err(err.into());
    }

    println!("Ran simulation. Time now {}ns", clock.time_now_ns());

    if let Err(err) = timetable.check_tasks_complete() {
        write_error_mermaid(&timetable, &args.error_mermaid.unwrap());
        return Err(err.into());
    }

    if dump_stats {
        timetable.dump_stats()?;
        platform.try_dump_stats(clock.time_now_ns())?;
    }

    Ok(())
}
