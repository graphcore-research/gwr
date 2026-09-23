// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A simple front-end for running a [Timetable] on a [Platform].

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use clap::Parser;
use gwr_engine::engine::Engine;
use gwr_engine::executor::ExecutorStats;
use gwr_engine::time::clock::Clock;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::Track;
use gwr_track::builder::{TrackerArgs, setup_trackers};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn progress_message(actual: usize, expected: usize, unit: &str) -> String {
    if expected == 0 {
        format!("{actual} / {expected} {unit} (n/a)")
    } else {
        format!(
            "{actual} / {expected} {unit} ({:.1}%)",
            actual as f64 / expected as f64 * 100.0
        )
    }
}

fn combine_run_and_progress_results(
    run_result: gwr_engine::types::SimResult,
    progress_result: gwr_engine::types::SimResult,
) -> gwr_engine::types::SimResult {
    run_result.and(progress_result)
}

fn activity_message(
    active: usize,
    stats: ExecutorStats,
    active_task_count: usize,
    simulated_ns: f64,
) -> String {
    format!(
        "{active:>6} active | {:>6} executor tasks | {} polls, {} completed | {simulated_ns:.3} ns simulated",
        active_task_count, stats.futures_polled, stats.futures_completed,
    )
}

#[derive(Clone)]
struct ProgressBars {
    _multi: MultiProgress,
    totals: gwr_timetable::WorkloadTotals,
    nodes: ProgressBar,
    reads: ProgressBar,
    writes: ProgressBar,
    compute: ProgressBar,
    activity: ProgressBar,
}

impl ProgressBars {
    fn new(totals: gwr_timetable::WorkloadTotals) -> Self {
        Self::with_draw_target(totals, ProgressDrawTarget::stderr())
    }

    fn with_draw_target(
        totals: gwr_timetable::WorkloadTotals,
        draw_target: ProgressDrawTarget,
    ) -> Self {
        let multi = MultiProgress::with_draw_target(draw_target);
        let bar_style =
            ProgressStyle::with_template("{prefix:>12} [{wide_bar:.cyan/blue}] {msg:<50!}")
                .unwrap()
                .progress_chars("=>-");

        let add_bar = |prefix: &'static str, total: usize| {
            let bar = multi.add(ProgressBar::new(total as u64));
            bar.set_style(bar_style.clone());
            bar.set_prefix(prefix);
            bar
        };

        let activity = multi.add(ProgressBar::new_spinner());
        activity.set_style(
            ProgressStyle::with_template("{spinner:.green} {prefix:>12}: {msg}").unwrap(),
        );
        activity.set_prefix("activity");

        Self {
            totals,
            nodes: add_bar("nodes", totals.total_tasks),
            reads: add_bar("reads", totals.read_bytes),
            writes: add_bar("writes", totals.written_bytes),
            compute: add_bar("compute", totals.machine_operations),
            activity,
            _multi: multi,
        }
    }

    fn update(
        &self,
        timetable: &Timetable,
        clock: &Clock,
        executor_stats: ExecutorStats,
        active_task_count: usize,
    ) -> gwr_engine::types::SimResult {
        let completed = timetable.completed_workload_totals()?;

        Self::update_bar(
            &self.nodes,
            completed.total_tasks,
            self.totals.total_tasks,
            "nodes",
        );
        Self::update_bar(
            &self.reads,
            completed.read_bytes,
            self.totals.read_bytes,
            "bytes",
        );
        Self::update_bar(
            &self.writes,
            completed.written_bytes,
            self.totals.written_bytes,
            "bytes",
        );
        Self::update_bar(
            &self.compute,
            completed.machine_operations,
            self.totals.machine_operations,
            "ops",
        );

        self.update_activity(
            timetable.num_graph_nodes_active(),
            executor_stats,
            active_task_count,
            clock.time_now_ns(),
        );
        Ok(())
    }

    fn update_activity(
        &self,
        active: usize,
        executor_stats: ExecutorStats,
        active_task_count: usize,
        simulated_ns: f64,
    ) {
        self.activity.set_message(activity_message(
            active,
            executor_stats,
            active_task_count,
            simulated_ns,
        ));
        self.activity.tick();
    }

    fn update_bar(bar: &ProgressBar, actual: usize, expected: usize, unit: &str) {
        bar.set_position(actual as u64);
        bar.set_message(progress_message(actual, expected, unit));
    }

    fn finish(&self) {
        self.nodes.finish();
        self.reads.finish();
        self.writes.finish();
        self.compute.finish();
        self.activity.finish();
    }
}

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Run a timetable on a platform and optionally emit traces and summary stats")]
struct Cli {
    #[command(flatten)]
    tracker: TrackerArgs,

    /// Show runtime progress for completed nodes, memory traffic, compute,
    /// active timetable nodes and executor activity. Executor task counts do
    /// not include futures parked on clocks, ports, or events.
    /// Updated at the rate defined by `progress_events`.
    #[arg(long)]
    progress: bool,

    /// Number of future polls between updates to the progress display. Only
    /// used when `progress` is enabled.
    #[arg(long, default_value = "1000000")]
    progress_events: usize,

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

fn futures_per_second(futures_completed: usize, elapsed: Duration) -> f64 {
    if elapsed.is_zero() {
        0.0
    } else {
        futures_completed as f64 / elapsed.as_secs_f64()
    }
}

fn executor_performance_message(stats: ExecutorStats, elapsed: Duration) -> String {
    format!(
        "{} async futures completed in {:.3}s ({:.1} futures/s wall-clock, {} polls)",
        stats.futures_completed,
        elapsed.as_secs_f64(),
        futures_per_second(stats.futures_completed, elapsed),
        stats.futures_polled,
    )
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
    let graph = timetable_file.into_graph()?;

    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform)?);
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);

    println!("Loaded timetable with {num_nodes} nodes, {num_edges} edges.");

    let mut progress_bars = None;
    if args.progress {
        let bars = ProgressBars::new(timetable.workload_totals()?);
        progress_bars = Some(bars);
    }

    let run_started = Instant::now();
    let run_result = if let Some(progress_bars) = &progress_bars {
        engine.run_with_executor_observer(args.progress_events, |snapshot| {
            progress_bars.update(
                &timetable,
                &clock,
                snapshot.stats,
                snapshot.active_task_count,
            )?;
            Ok(())
        })
    } else {
        engine.run()
    };
    let run_elapsed = run_started.elapsed();
    let executor_stats = engine.executor_stats();

    let progress_result = progress_bars.map_or(Ok(()), |progress_bars| {
        let result = progress_bars.update(&timetable, &clock, executor_stats, 0);
        progress_bars.finish();
        result
    });

    if run_result.is_err() {
        write_error_mermaid(&timetable, &args.error_mermaid);
    }
    combine_run_and_progress_results(run_result, progress_result)?;

    println!("Ran simulation. Time now {}ns", clock.time_now_ns());
    println!(
        "{}",
        executor_performance_message(executor_stats, run_elapsed)
    );

    if let Err(err) = timetable.check_tasks_complete() {
        write_error_mermaid(&timetable, &args.error_mermaid);
        return Err(err.into());
    }

    if args.dump_stats {
        timetable.dump_stats()?;
        platform.try_dump_stats(clock.time_now_ns())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gwr_engine::executor::ExecutorStats;

    use super::*;

    #[test]
    fn progress_events_default_to_a_sparse_interval() {
        let args = Cli::try_parse_from(["gwr-timetable"]).unwrap();
        let progress_events: usize = args.progress_events;

        assert_eq!(progress_events, 1_000_000);
    }

    #[test]
    fn progress_events_leave_zero_validation_to_the_engine() {
        let args = Cli::try_parse_from(["gwr-timetable", "--progress-events", "0"]).unwrap();

        assert_eq!(args.progress_events, 0);
    }

    #[test]
    fn ordinary_run_does_not_enable_info_tracking() {
        let mut args = Cli::try_parse_from(["gwr-timetable"]).unwrap();

        args.tracker
            .ensure_visiblity(args.dump_stats, "--dump-stats", log::Level::Info);

        assert!(!args.tracker.level_enabled(log::Level::Info));
    }

    #[test]
    fn dump_stats_enables_info_tracking() {
        let mut args = Cli::try_parse_from(["gwr-timetable", "--dump-stats"]).unwrap();

        args.tracker
            .ensure_visiblity(args.dump_stats, "--dump-stats", log::Level::Info);

        assert!(args.tracker.level_enabled(log::Level::Info));
    }

    #[test]
    fn executor_rate_is_zero_when_no_wall_clock_time_elapsed() {
        assert_eq!(futures_per_second(123, Duration::ZERO), 0.0);
    }

    #[test]
    fn executor_performance_is_formatted_for_stdout() {
        assert_eq!(
            executor_performance_message(
                ExecutorStats {
                    futures_polled: 80,
                    futures_completed: 50,
                },
                Duration::from_millis(250),
            ),
            "50 async futures completed in 0.250s (200.0 futures/s wall-clock, 80 polls)",
        );
    }
}
