// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a device comprising ring nodes.
//!
//! See `lib.rs` for details.
use std::rc::Rc;

use gwr_components::cli::{deserialize_optional_bytes, parse_bytes_string};
use gwr_components::connect_port;
use gwr_config::multi_source_config;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::SimError;
use gwr_engine::{run_simulation, sim_error};
use gwr_track::builder::{TrackerArgs, TrackerArgsPartial, setup_trackers};
use gwr_track::{Track, error, info};
use indicatif::ProgressBar;
use sim_ring::ring_builder::{
    Config, Sinks, build_limiters, build_pipes, build_ring_nodes, build_source_sinks,
};

// Define the standard Ethernet data rate
const ETHERNET_GBPS: usize = 100;

/// Command-line arguments.
#[multi_source_config(default_conf_file = "sim-ring.toml", conf_file_flag = "conf-file")]
#[derive(PartialEq)]
#[command(about = "Ring deadlock application")]
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

    /// The number of nodes in the ring.
    #[arg(long, default_value_t = 8)]
    ring_size: Option<usize>,

    /// The number of bytes to send from each source.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = parse_bytes_string("100KiB").unwrap())]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    bytes_to_send: Option<usize>,

    /// The priority of ring traffic over local traffic in the arbiter.
    #[arg(long, default_value_t = 1)]
    ring_priority: Option<usize>,

    /// Override the default number of bytes in the Tx buffer.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = parse_bytes_string("32KiB").unwrap())]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    tx_buffer_bytes: Option<usize>,

    /// Override the default number of bytes in the Rx buffer.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = parse_bytes_string("32KiB").unwrap())]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    rx_buffer_bytes: Option<usize>,

    /// Override the default frame payload bytes.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = 256)]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    frame_payload_bytes: Option<usize>,
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
            // Use the `background` wait to indicate that the simulation can end
            // if this is the only task still active.
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

fn main() -> Result<(), SimError> {
    let args = Cli::parse_all_sources();
    let tracker: Rc<dyn Track> = setup_trackers(&args.tracker.trackers_config()).unwrap();

    let mut engine = Engine::new(&tracker);
    let spawner = engine.spawner();
    let clock = engine.default_clock();

    let config = Config {
        ring_size: args.ring_size.unwrap(),
        ring_priority: args.ring_priority.unwrap(),
        rx_buffer_bytes: args.rx_buffer_bytes.unwrap(),
        tx_buffer_bytes: args.tx_buffer_bytes.unwrap(),
        frame_payload_bytes: args.frame_payload_bytes.unwrap(),
        num_send_frames: args.bytes_to_send.unwrap() / args.frame_payload_bytes.unwrap(),
    };

    let top = engine.top().clone();
    info!(top ;
        "Ring of {} sources, priority {}, each sending {} frames ({} bytes) with buffers {}/{} bytes.",
        config.ring_size,
        config.ring_priority,
        config.num_send_frames,
        args.bytes_to_send.unwrap(),
        args.rx_buffer_bytes.unwrap(),
        args.tx_buffer_bytes.unwrap()
    );

    let ring_nodes = build_ring_nodes(&mut engine, &clock, &config);
    let (sources, sinks) = build_source_sinks(&mut engine, &clock, &config);
    let (ingress_pipes, ring_pipes) = build_pipes(&mut engine, &clock, &config);
    let (source_limiters, ring_limiters, sink_limiters) =
        build_limiters(&mut engine, &clock, &config, ETHERNET_GBPS);

    for i in 0..config.ring_size {
        let right = (i + 1) % config.ring_size;

        // Connect the sources to the ring using a rater limiter and flow
        // controlled pipeline.
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
    let mut progress_bar = None;
    if args.progress.unwrap() {
        progress_bar = Some(ProgressBar::new(total_expected_frames as u64));
        let sinks = sinks.to_owned();
        start_frame_dump(
            &spawner,
            clock.clone(),
            args.progress_ticks.unwrap(),
            total_expected_frames,
            sinks,
            progress_bar.clone().unwrap(),
        );
    }

    let finish_tick = args.finish_tick.unwrap();
    if finish_tick != 0 {
        finish_at(&spawner, clock.clone(), finish_tick);
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
    if let Some(progress_bar) = progress_bar {
        progress_bar.finish();
    }
    info!(top ; "Pass ({:.2}ns)", clock.time_now_ns());
    Ok(())
}
