// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a device comprising ring nodes.
//!
//! See `lib.rs` for details.
use std::rc::Rc;

use clap::Parser;
use gwr_components::connect_port;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimError;
use gwr_engine::{run_simulation, sim_error};
use gwr_models::ethernet_frame::FRAME_OVERHEAD_BYTES;
use gwr_track::builder::{MonitorsConfig, TrackerConfig, TrackersConfig, setup_trackers};
use gwr_track::{Track, error, info};
use indicatif::ProgressBar;
use sim_ring::ring_builder::{
    Config, Sinks, build_limiters, build_pipes, build_ring_nodes, build_source_sinks,
};

// Define the standard Ethernet data rate
const ETHERNET_GBPS: usize = 100;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Ring deadlock application")]
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

    /// Configure a clock tick on which to terminate the simulation. Use 0 to
    /// run until completion.
    #[arg(long, default_value = "0")]
    finish_tick: usize,

    /// The number of nodes in the ring.
    #[arg(long, default_value = "8")]
    ring_size: usize,

    /// The number of KiB to send from each source.
    #[arg(long, default_value = "100")]
    kib_to_send: usize,

    /// The priority of ring traffic over local traffic in the arbiter.
    #[arg(long, default_value = "1")]
    ring_priority: usize,

    /// Override the default number of KiB in the Tx buffer.
    #[arg(long, default_value = "32")]
    tx_buffer_kib: usize,

    /// Override the default number of KiB in the Rx buffer.
    #[arg(long, default_value = "32")]
    rx_buffer_kib: usize,

    /// Override the default frame payload bytes.
    #[arg(long, default_value = "256")]
    frame_payload_bytes: usize,
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
    sinks: Sinks,
    progress_bar: ProgressBar,
) {
    spawner.spawn(async move {
        let mut seen_frames = 0;
        loop {
            // Use the `background` wait to indicate that the simulation can end if this is
            // the only task still active.
            clock.wait_ticks_or_exit(progress_ticks as u64).await;
            let num_frames: usize = sinks.iter().map(|s| s.num_sunk()).sum();
            progress_bar.inc((num_frames - seen_frames) as u64);
            seen_frames = num_frames;
            if num_frames == total_expected_frames {
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

fn main() -> Result<(), SimError> {
    let args = Cli::parse();

    let tracker = setup_all_trackers(&args);

    let mut engine = Engine::new(&tracker);
    let spawner = engine.spawner();
    let clock = engine.default_clock();

    let tx_buffer_bytes = args.tx_buffer_kib * 1024;
    let rx_buffer_bytes = args.rx_buffer_kib * 1024;
    let num_payload_bytes_to_send = args.kib_to_send * 1024;

    // Size of max-sized frames
    let frame_bytes = args.frame_payload_bytes + FRAME_OVERHEAD_BYTES;

    let config = Config {
        ring_size: args.ring_size,
        ring_priority: args.ring_priority,
        rx_buffer_frames: rx_buffer_bytes / frame_bytes,
        tx_buffer_frames: tx_buffer_bytes / frame_bytes,
        frame_payload_bytes: args.frame_payload_bytes,
        num_send_frames: num_payload_bytes_to_send / args.frame_payload_bytes,
    };

    let top = engine.top().clone();
    info!(top ;
        "Ring of {} sources, priority {}, each sending {} frames ({}KiB) with buffers {}/{} frames.",
        config.ring_size,
        config.ring_priority,
        config.num_send_frames,
        args.kib_to_send,
        config.rx_buffer_frames,
        config.tx_buffer_frames
    );

    let ring_nodes = build_ring_nodes(&mut engine, &clock, &config);
    let (sources, sinks) = build_source_sinks(&mut engine, &clock, &config);
    let (ingress_pipes, ring_pipes) = build_pipes(&mut engine, &clock, &config);
    let (source_limiters, ring_limiters, sink_limiters) =
        build_limiters(&mut engine, &clock, &config, ETHERNET_GBPS);

    for i in 0..config.ring_size {
        let right = (i + 1) % config.ring_size;

        // Connect the sources to the ring using a rater limiter and flow controlled
        // pipeline.
        connect_port!(sources[i], tx => source_limiters[i], rx)?;
        connect_port!(source_limiters[i], tx => ingress_pipes[i], rx)?;
        connect_port!(ingress_pipes[i], tx => ring_nodes[i], io_rx)?;

        // Connect the ring together using a rate limiter and a flow controlled
        // pipeline.
        connect_port!(ring_nodes[i], ring_tx => ring_limiters[i], rx)?;
        connect_port!(ring_limiters[i], tx => ring_pipes[i], rx)?;
        connect_port!(ring_pipes[i], tx => ring_nodes[right], ring_rx)?;

        // Connect the ring to the sinks using a rate limiter.
        connect_port!(ring_nodes[i], io_tx => sink_limiters[i], rx)?;
        connect_port!(sink_limiters[i], tx => sinks[i], rx)?;
    }

    info!(top ; "Platform built and connected");

    let total_expected_frames = config.num_send_frames * config.ring_size;
    let progress_bar = ProgressBar::new(total_expected_frames as u64);
    if args.progress {
        let sinks = sinks.to_owned();
        start_frame_dump(
            &spawner,
            clock.clone(),
            args.progress_ticks,
            total_expected_frames,
            sinks,
            progress_bar.clone(),
        );
    }

    if args.finish_tick != 0 {
        finish_at(&spawner, clock.clone(), args.finish_tick);
    }

    run_simulation!(engine);

    for sink in &sinks {
        if sink.num_sunk() != config.num_send_frames {
            error!(top ; "{}/{} frames received", sink.num_sunk(), config.num_send_frames);
            error!(top ; "Deadlock detected at {:.2}ns", clock.time_now_ns());

            tracker.shutdown();
            return sim_error!("Deadlock");
        }
    }
    if args.progress {
        progress_bar.finish();
    }
    info!(top ; "Pass ({:.2}ns)", clock.time_now_ns());
    Ok(())
}
