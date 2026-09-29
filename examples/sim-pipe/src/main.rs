// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a flow-controlled pipeline.
//!
//! See `lib.rs` for details.
use std::rc::Rc;

use byte_unit::{AdjustedByte, Byte, UnitType};
use gwr_components::cli::{deserialize_optional_bytes, parse_bytes_string};
use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, rc_limiter};
use gwr_config::multi_source_config;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimError;
use gwr_engine::{run_simulation, sim_error};
use gwr_models::fc_pipeline::{FcPipeline, FcPipelineConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_track::builder::{TrackerArgs, TrackerArgsPartial, setup_trackers};
use gwr_track::entity::Entity;
use gwr_track::{Track, error, info};
use indicatif::ProgressBar;
use sim_pipe::frame_gen::FrameGen;

/// Command-line arguments.
#[multi_source_config(default_conf_file = "sim-pipe.toml", conf_file_flag = "conf-file")]
#[derive(PartialEq)]
#[command(about = "Flow controlled evaluation application")]
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

    /// Configure a clock tick on which to terminate the simulation. Use 0 to
    /// run until completion.
    #[arg(long, default_value_t = 0)]
    finish_tick: Option<usize>,

    /// The number of bytes to send from each source.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = parse_bytes_string("100KiB").unwrap())]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    bytes_to_send: Option<usize>,

    /// Set the frame overhead (protocol) bytes.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = 8)]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    frame_overhead_bytes: Option<usize>,

    /// Set the frame payload bytes.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = 8)]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    frame_payload_bytes: Option<usize>,

    /// Set many bits per clock tick the RX port can accept.
    #[arg(long, default_value_t = 128)]
    pipe_rx_bits_per_tick: Option<usize>,

    /// Set many bits per clock tick the TX port will send.
    #[arg(long, default_value_t = 128)]
    pipe_tx_bits_per_tick: Option<usize>,

    /// Set the number of frames the pipe buffer can hold
    #[arg(long, default_value_t = 10)]
    pipe_buffer_entries: Option<usize>,

    /// Set the number of ticks it takes for data to travel through the
    /// pipeline
    #[arg(long, default_value_t = 5)]
    pipe_data_delay: Option<usize>,

    /// Set the number of ticks it takes for credit to be returned to the start
    /// of the pipe
    #[arg(long, default_value_t = 5)]
    pipe_credit_delay: Option<usize>,
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
    sink: Rc<Sink<MemoryAccess>>,
    progress_bar: ProgressBar,
) {
    spawner.spawn(async move {
        let mut seen_frames = 0;
        loop {
            // Use the `background` wait to indicate that the simulation can end
            // if this is the only task still active.
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

fn main() -> Result<(), SimError> {
    let args = Cli::parse_all_sources();
    let tracker: Rc<dyn Track> = setup_trackers(&args.tracker.trackers_config()).unwrap();

    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let spawner = engine.spawner();

    let bytes_to_send = args.bytes_to_send.unwrap();
    let frame_overhead_bytes = args.frame_overhead_bytes.unwrap();
    let frame_payload_bytes = args.frame_payload_bytes.unwrap();
    let num_send_frames = bytes_to_send / frame_payload_bytes;
    let total_expected_frames = num_send_frames;

    let top = engine.top().clone();
    info!(top ;
        "Sending {} frames ({} bytes) through pipe with: data delay={}, credit delay={}, buffer entries={}, rx={}bps, tx={}bps.",
        num_send_frames,
        bytes_to_send,
        args.pipe_data_delay.unwrap(),
        args.pipe_credit_delay.unwrap(),
        args.pipe_buffer_entries.unwrap(),
        args.pipe_rx_bits_per_tick.unwrap(),
        args.pipe_tx_bits_per_tick.unwrap(),
    );

    let frame_gen = FrameGen::new(
        &top,
        frame_overhead_bytes,
        frame_payload_bytes,
        num_send_frames,
    );
    let source = Source::new_and_register(&engine, &top, "source", Some(Box::new(frame_gen)));
    let rx_limiter = rc_limiter!(&clock, args.pipe_rx_bits_per_tick.unwrap());
    let source_limiter = Limiter::new_and_register(&engine, &clock, &top, "rx_limiter", rx_limiter);

    let pipe_config = FcPipelineConfig::new(
        args.pipe_buffer_entries.unwrap(),
        args.pipe_data_delay.unwrap(),
        args.pipe_credit_delay.unwrap(),
    );
    let pipe = FcPipeline::new_and_register(&engine, &clock, &top, "pipe", &pipe_config)?;
    let tx_limiter = rc_limiter!(&clock, args.pipe_tx_bits_per_tick.unwrap());
    let sink_limiter = Limiter::new_and_register(&engine, &clock, &top, "tx_limiter", tx_limiter);
    let sink = Sink::new_and_register(&engine, &clock, &top, "sink");

    connect_port!(source, tx => source_limiter, rx)?;
    connect_port!(source_limiter, tx => pipe, rx)?;
    connect_port!(pipe, tx => sink_limiter, rx)?;
    connect_port!(sink_limiter, tx => sink, rx)?;

    info!(top ; "Platform built and connected");

    let mut progress_bar = None;
    if args.progress.unwrap() {
        progress_bar = Some(ProgressBar::new(num_send_frames as u64));
        let sink = sink.clone();
        start_frame_dump(
            &spawner,
            clock.clone(),
            args.progress_ticks.unwrap(),
            total_expected_frames,
            sink,
            progress_bar.clone().unwrap(),
        );
    }

    let finish_tick = args.finish_tick.unwrap();
    if finish_tick != 0 {
        finish_at(&spawner, clock.clone(), finish_tick);
    }

    run_simulation!(engine);

    let total_sunk_frames = sink.num_sunk();
    if total_sunk_frames != total_expected_frames {
        error!(top ; "{}/{} frames received", total_sunk_frames, total_expected_frames);
        error!(top ; "Deadlock detected at {:.2}ns", clock.time_now_ns());

        tracker.shutdown();
        return sim_error!("Deadlock");
    }

    if let Some(progress_bar) = progress_bar {
        progress_bar.finish();
    }

    print_summary(
        &top,
        clock.time_now_ns(),
        total_sunk_frames,
        frame_overhead_bytes,
        frame_payload_bytes,
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
