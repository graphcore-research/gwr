// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a flow-controlled pipeline.
//!
//! See `lib.rs` for details.
use std::rc::Rc;

use byte_unit::{AdjustedByte, Byte, UnitType};
use clap::Parser;
use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, rc_limiter};
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimError;
use gwr_engine::{run_simulation, sim_error};
use gwr_models::data_frame::DataFrame;
use gwr_models::fc_pipeline::{FcPipeline, FcPipelineConfig};
use gwr_track::builder::setup_all_trackers;
use gwr_track::entity::Entity;
use gwr_track::{Track, error, info};
use indicatif::ProgressBar;
use sim_pipe::frame_gen::FrameGen;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Flow controlled evaluation application")]
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

    /// Show a progress bar for the received frame count (updated at the rate
    /// defined by `progress_ticks`).
    #[arg(long)]
    progress: bool,

    /// Number of ticks between updates to the progress bar. Only used when
    /// `progress` is enabled.
    #[arg(long, default_value = "1000")]
    progress_ticks: usize,

    /// Configure a clock tick on which to terminate the simulation. Use 0 to
    /// run until completion.
    #[arg(long, default_value = "0")]
    finish_tick: usize,

    /// The number of KiB to send from each source.
    #[arg(long, default_value = "100")]
    kib_to_send: usize,

    /// Set the frame overhead (protocol) bytes.
    #[arg(long, default_value = "8")]
    frame_overhead_bytes: usize,

    /// Set the frame payload bytes.
    #[arg(long, default_value = "8")]
    frame_payload_bytes: usize,

    /// Set many bits per clock tick the RX port can accept.
    #[arg(long, default_value = "128")]
    pipe_rx_bits_per_tick: usize,

    /// Set many bits per clock tick the TX port will send.
    #[arg(long, default_value = "128")]
    pipe_tx_bits_per_tick: usize,

    /// Set the number of frames the pipe buffer can hold
    #[arg(long, default_value = "10")]
    pipe_buffer_entries: usize,

    /// Set the number of cycles it takes for data to travel through the
    /// pipeline
    #[arg(long, default_value = "5")]
    pipe_data_delay: usize,

    /// Set the number of cycles it takes for credit to be returned to the start
    /// of the pipe
    #[arg(long, default_value = "5")]
    pipe_credit_delay: usize,
}

/// Install an event to terminate the simulation at the clock tick defined.
fn finish_at(spawner: &Spawner, clock: Clock, run_ticks: usize) {
    spawner.spawn(async move {
        clock.wait_ticks(run_ticks as u64).await;
        sim_error!("Finish")
    });
}

/// Spawn a background task to display regular updates of the total number of
/// frames received so far.
fn start_frame_dump(
    spawner: &Spawner,
    clock: Clock,
    progress_ticks: usize,
    total_expected_frames: usize,
    sink: Rc<Sink<DataFrame>>,
    progress_bar: ProgressBar,
) {
    spawner.spawn(async move {
        let mut seen_frames = 0;
        loop {
            // Use the `background` wait to indicate that the simulation can end if this is
            // the only task still active.
            clock.wait_ticks_or_exit(progress_ticks as u64).await;
            let num_frames = sink.num_sunk();
            progress_bar.inc((num_frames - seen_frames) as u64);
            seen_frames = num_frames;
            if num_frames == total_expected_frames {
                break;
            }
        }
        Ok(())
    });
}

fn setup_trackers(args: &Cli) -> Rc<dyn Track> {
    setup_all_trackers(
        args.stdout,
        args.stdout_level,
        args.stdout_filter_regex.as_str(),
        args.binary,
        args.binary_level,
        &args.binary_filter_regex,
        &args.binary_file,
        args.perfetto,
        args.perfetto_level,
        &args.perfetto_filter_regex,
        &args.perfetto_file,
    )
    .unwrap()
}

fn main() -> Result<(), SimError> {
    let args = Cli::parse();
    let tracker = setup_trackers(&args);

    let mut engine = Engine::new(&tracker);
    let spawner = engine.spawner();
    let clock = engine.default_clock();

    let num_payload_bytes_to_send = args.kib_to_send * 1024;
    let num_send_frames = num_payload_bytes_to_send / args.frame_payload_bytes;
    let total_expected_frames = num_send_frames;

    let top = engine.top().clone();
    info!(top ;
        "Sending {} frames ({}KiB) through pipe with: data delay={}, credit delay={}, buffer entries={}, rx={}bps, tx={}bps.",
        num_send_frames,
        args.kib_to_send,
        args.pipe_data_delay,
        args.pipe_credit_delay,
        args.pipe_buffer_entries,
        args.pipe_rx_bits_per_tick,
        args.pipe_tx_bits_per_tick,
    );

    let frame_gen = FrameGen::new(
        &top,
        args.frame_overhead_bytes,
        args.frame_payload_bytes,
        num_send_frames,
    );
    let source = Source::new_and_register(&engine, &top, "source", Some(Box::new(frame_gen)))?;
    let rx_limiter = rc_limiter!(clock.clone(), args.pipe_rx_bits_per_tick);
    let source_limiter = Limiter::new_and_register(&engine, &top, "rx_limiter", rx_limiter)?;

    let pipe_config = FcPipelineConfig::new(
        args.pipe_buffer_entries,
        args.pipe_data_delay,
        args.pipe_credit_delay,
    );
    let pipe = FcPipeline::new_and_register(
        &engine,
        &top,
        "pipe",
        clock.clone(),
        spawner.clone(),
        &pipe_config,
    )?;
    let tx_limiter = rc_limiter!(clock.clone(), args.pipe_tx_bits_per_tick);
    let sink_limiter = Limiter::new_and_register(&engine, &top, "tx_limiter", tx_limiter)?;
    let sink =
        Sink::new_and_register(&engine, &top, "sink").expect("should be able to create sink");

    connect_port!(source, tx => source_limiter, rx)?;
    connect_port!(source_limiter, tx => pipe, rx)?;
    connect_port!(pipe, tx => sink_limiter, rx)?;
    connect_port!(sink_limiter, tx => sink, rx)?;

    info!(top ; "Platform built and connected");

    let progress_bar = ProgressBar::new(num_send_frames as u64);
    if args.progress {
        let sink = sink.clone();
        start_frame_dump(
            &spawner,
            clock.clone(),
            args.progress_ticks,
            total_expected_frames,
            sink,
            progress_bar.clone(),
        );
    }

    if args.finish_tick != 0 {
        finish_at(&spawner, clock.clone(), args.finish_tick);
    }

    run_simulation!(engine);

    let total_sunk_frames = sink.num_sunk();
    if total_sunk_frames != total_expected_frames {
        error!(top ; "{}/{} frames received", total_sunk_frames, total_expected_frames);
        error!(top ; "Deadlock detected at {:.2}ns", clock.time_now_ns());

        tracker.shutdown();
        return sim_error!("Deadlock");
    }

    if args.progress {
        progress_bar.finish();
    }

    print_summary(
        &top,
        clock.time_now_ns(),
        total_sunk_frames,
        args.frame_overhead_bytes,
        args.frame_payload_bytes,
    );
    Ok(())
}

fn print_summary(
    top: &Rc<Entity>,
    time_now_ns: f64,
    total_sunk_frames: usize,
    frame_overhead_bytes: usize,
    frame_payload_bytes: usize,
) {
    let time_now_s = time_now_ns / (1000.0 * 1000.0 * 1000.0);

    let payload_bytes = (total_sunk_frames * frame_payload_bytes) as u64;
    let (payload_value, payload_per_second) =
        compute_adjusted_value_and_rate(time_now_s, payload_bytes);

    let total_bytes = payload_bytes + (total_sunk_frames * frame_overhead_bytes) as u64;
    let (total_value, total_per_second) = compute_adjusted_value_and_rate(time_now_s, total_bytes);

    info!(top ; "Pass: Sent {total_sunk_frames} in {time_now_ns:.2}ns.");
    info!(top ; "Payload: {payload_value:.2} ({payload_per_second:.2}/s). Total: {total_value:.2} ({total_per_second:.2}/s).");
}

fn compute_adjusted_value_and_rate(
    time_now_s: f64,
    num_bytes: u64,
) -> (AdjustedByte, AdjustedByte) {
    // Convert to a binary-only unit (KiB, MiB, etc)
    let count = Byte::from_u64(num_bytes).get_appropriate_unit(UnitType::Binary);
    let per_second = Byte::from_f64(num_bytes as f64 / time_now_s).unwrap();
    let count_per_second = per_second.get_appropriate_unit(UnitType::Binary);
    (count, count_per_second)
}
