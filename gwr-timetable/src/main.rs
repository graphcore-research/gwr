// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A simple front-end for running a [Timetable] on a [Platform].

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use clap::Parser;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::Track;
use gwr_track::builder::{TrackerArgs, setup_trackers};
use indicatif::ProgressBar;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Run a timetable on a platform and optionally emit traces and summary stats")]
struct Cli {
    #[command(flatten)]
    tracker: TrackerArgs,

    /// Show a progress bar for the received frame count (updated at the rate
    /// defined by `progress_ticks`).
    #[arg(long)]
    progress: bool,

    /// Number of ticks between updates to the progress bar. Only used when
    /// `progress` is enabled.
    #[arg(long, default_value = "1000")]
    progress_ticks: usize,

    /// Timetable YAML file
    #[arg(long, default_value = "timetable.yaml")]
    timetable: PathBuf,

    /// Platform YAML file
    #[arg(long, default_value = "platform.yaml")]
    platform: PathBuf,

    /// Enable dumping of summary statistics
    #[arg(long, default_value = "false")]
    dump_stats: bool,

    /// Write a Mermaid diagram of the timetable state to this file if execution
    /// fails.
    #[arg(long, default_value = "error.mmd")]
    error_mermaid: PathBuf,
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
            // Use the `background` wait to indicate that the simulation can end if this is
            // the only task still active.
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

fn main() -> Result<()> {
    let mut args = Cli::parse();
    args.tracker
        .ensure_visiblity(args.dump_stats, "--dump-stats", log::Level::Info);

    let tracker: Rc<dyn Track> = setup_trackers(&args.tracker.trackers_config()).unwrap();
    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_file(
        &engine,
        &clock,
        Path::new(&args.platform),
    )?);

    println!("Loaded platform:\n{platform}");

    let timetable_file = TimetableFile::from_file(&args.timetable)?;
    let num_nodes = timetable_file.nodes.len();
    let num_edges = timetable_file.edges.len();

    let timetable = Rc::new(Timetable::new(engine.top(), timetable_file, &platform)?);
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);

    println!("Loaded timetable with {num_nodes} nodes, {num_edges} edges.");

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

    let run_result = engine.run();

    if let Some(progress_bar) = progress_bar {
        progress_bar.finish();
    }

    if let Err(err) = run_result {
        write_error_mermaid(&timetable, &args.error_mermaid);
        return Err(err.into());
    }

    println!("Ran simulation. Time now {clock:.2}");

    if let Err(err) = timetable.check_tasks_complete() {
        write_error_mermaid(&timetable, &args.error_mermaid);
        return Err(err.into());
    }

    if args.dump_stats {
        timetable.dump_stats()?;
        platform.dump_stats(clock.time_now());
    }

    Ok(())
}
