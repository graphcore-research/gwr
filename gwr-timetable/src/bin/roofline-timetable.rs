// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use clap::Parser;
use gwr_engine::engine::Engine;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::Timetable;
use gwr_timetable::analysis::report::{ReportOptions, print_roofline_report};
use gwr_timetable::analysis::roofline::{RooflineAnalysisOptions, RooflineAnalyzer};
use gwr_timetable::timetable_file::TimetableFile;
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

    /// Print ranked compute-node roofline reports.
    #[arg(long, default_value_t = false)]
    node_rankings: bool,

    /// Print per-PE roofline aggregate totals.
    #[arg(long, default_value_t = false)]
    pe_summary: bool,

    /// Print scheduled per-PE activity timelines.
    #[arg(long, default_value_t = false)]
    activity_report: bool,

    /// Print per-memory oversubscription windows.
    #[arg(long, default_value_t = false)]
    memory_report: bool,

    /// Skip the post-memory scheduled runtime in the runtime summary.
    #[arg(
        long = "no-scheduled-runtime",
        alias = "no-scheuled-runtime",
        default_value_t = false
    )]
    no_scheduled_runtime: bool,

    /// Include the nodes on the compute critical path in the runtime summary.
    #[arg(long, default_value_t = false)]
    critical_path_nodes: bool,

    /// Print every report section.
    #[arg(long, default_value_t = false)]
    full_report: bool,

    /// Enable debug output for internal analysis steps.
    #[arg(long, default_value_t = false)]
    debug: bool,

    /// Include explicit dependency lists in the per-PE activity report.
    #[arg(long, default_value_t = false)]
    show_deps: bool,

    /// Print coarse phase timings to stderr.
    #[arg(long, default_value_t = false)]
    timings: bool,
}

fn parse_platform_config(path: &PathBuf) -> Result<PlatformConfig> {
    let content = std::fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&content)?)
}

fn report_options(args: &Args) -> ReportOptions {
    if args.full_report {
        return ReportOptions::full(args.top, args.show_deps);
    }

    ReportOptions {
        top: args.top,
        node_rankings: args.node_rankings,
        pe_summary: args.pe_summary,
        activity_report: args.activity_report,
        memory_report: args.memory_report,
        scheduled_runtime: !args.no_scheduled_runtime,
        critical_path_nodes: args.critical_path_nodes,
        show_deps: args.show_deps,
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    init_logging(args.debug);
    let report_options = report_options(&args);

    let total_start = Instant::now();
    let phase_start = Instant::now();
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_file(&engine, &clock, &args.platform)?);
    let platform_cfg = parse_platform_config(&args.platform)?;
    if args.timings {
        eprintln!(
            "timing: load platform {:.3}s",
            phase_start.elapsed().as_secs_f64()
        );
    }

    let phase_start = Instant::now();
    let timetable_file = TimetableFile::from_file(&args.timetable)?;
    let timetable = Timetable::new(engine.top(), timetable_file, &platform)?;
    if args.timings {
        eprintln!(
            "timing: load timetable {:.3}s",
            phase_start.elapsed().as_secs_f64()
        );
    }

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

    let phase_start = Instant::now();
    let analyzer = RooflineAnalyzer::new(&platform, &platform_cfg)?;
    let report = analyzer.analyze_with_options(
        &platform,
        &timetable,
        RooflineAnalysisOptions {
            schedule_activities: report_options.needs_scheduled_activities(),
        },
    )?;
    if args.timings {
        eprintln!(
            "timing: analyse {:.3}s",
            phase_start.elapsed().as_secs_f64()
        );
    }

    let phase_start = Instant::now();
    print_roofline_report(&clock, &platform, &report, &report_options);
    if args.timings {
        eprintln!("timing: report {:.3}s", phase_start.elapsed().as_secs_f64());
        eprintln!("timing: total {:.3}s", total_start.elapsed().as_secs_f64());
    }

    Ok(())
}
