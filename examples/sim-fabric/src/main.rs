// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a device comprising a rectangular fabric.
//!
//! See `lib.rs` for details.
use std::rc::Rc;

use clap::Parser;
use gwr_components::connect_port;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_engine::time::compute_adjusted_value_and_rate;
use gwr_engine::types::SimError;
use gwr_engine::{run_simulation, sim_error};
use gwr_models::data_frame::DataFrame;
use gwr_models::fabric::functional::FunctionalFabric;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::fabric::routed::RoutedFabric;
use gwr_models::fabric::{Fabric, FabricConfig};
use gwr_track::builder::{TrackerArgs, setup_trackers};
use gwr_track::entity::Entity;
use gwr_track::{Track, error, info};
use indicatif::ProgressBar;
use sim_fabric::frame_gen::TrafficPattern;
use sim_fabric::source_sink_builder::{Sinks, build_source_sinks};

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Fabric evaluation application")]
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

    /// Configure a clock tick on which to terminate the simulation. Use 0 to
    /// run until completion.
    #[arg(long, default_value = "0")]
    finish_tick: usize,

    /// The number of columns in the fabric.
    #[arg(long, default_value = "4")]
    fabric_columns: usize,

    /// The number of rows in the fabric.
    #[arg(long, default_value = "3")]
    fabric_rows: usize,

    /// The number of ports at each node of the fabric.
    #[arg(long, default_value = "2")]
    fabric_ports_per_node: usize,

    /// The number of KiB to send from each source.
    #[arg(long, default_value = "100")]
    kib_to_send: usize,

    /// Set the number of frames each fabric TX port can hold.
    #[arg(long, default_value = "32")]
    tx_buffer_entries: usize,

    /// Set the number of frames each fabric RX port can hold.
    #[arg(long, default_value = "32")]
    rx_buffer_entries: usize,

    /// Set many bits per clock tick the fabric TX/RX ports move.
    #[arg(long, default_value = "128")]
    port_bits_per_tick: usize,

    /// Set the frame overhead (protocol) bytes.
    #[arg(long, default_value = "8")]
    frame_overhead_bytes: usize,

    /// Set the default frame payload bytes.
    #[arg(long, default_value = "32")]
    frame_payload_bytes: usize,

    /// Set the clock ticks required to move one hop in the fabric.
    #[arg(long, default_value = "1")]
    ticks_per_hop: usize,

    /// An extra overhead for every frame passing through the fabric.
    #[arg(long, default_value = "1")]
    ticks_overhead: usize,

    /// What traffic pattern to use.
    #[clap(long, default_value_t, value_enum)]
    traffic_pattern: TrafficPattern,

    /// Number of active sources (chosen at random from possible sources).
    #[clap(long)]
    active_sources: Option<usize>,

    /// Seed for random number generator.
    #[clap(long, default_value = "1")]
    seed: u64,

    /// Seed for random number generator.
    #[clap(long, default_value = "false")]
    routed: bool,

    /// Seed for random number generator.
    #[clap(long, default_value_t, value_enum)]
    fabric_routing: FabricRoutingAlgorithm,
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

fn create_config(engine: &Engine, args: &Cli) -> (Rc<FabricConfig>, usize) {
    let config = FabricConfig::new(
        args.fabric_columns,
        args.fabric_rows,
        args.fabric_ports_per_node,
        None,
        args.ticks_per_hop,
        args.ticks_overhead,
        args.rx_buffer_entries,
        args.tx_buffer_entries,
        args.port_bits_per_tick,
    );
    let config = Rc::new(config);

    let num_payload_bytes_to_send = args.kib_to_send * 1024;

    // Size of max-sized frames
    let num_send_frames = num_payload_bytes_to_send / args.frame_payload_bytes;

    let top = engine.top();
    info!(top ;
        "Fabric of {}x{}x{} sources, each sending {} frames ({}KiB) with buffers {}/{} frames.",
        config.num_columns(),
        config.num_rows(),
        config.num_ports_per_node(),
        num_send_frames,
        args.kib_to_send,
        args.rx_buffer_entries,
        args.tx_buffer_entries,
    );
    info!(top ; "Using traffic pattern {}. Random seed {}", args.traffic_pattern, args.seed);

    (config, num_send_frames)
}

fn main() -> Result<(), SimError> {
    let args = Cli::parse();
    let tracker: Rc<dyn Track> = setup_trackers(&args.tracker.trackers_config()).unwrap();

    let mut engine = Engine::new(&tracker);
    let spawner = engine.spawner();
    let clock = engine.default_clock();

    let (config, num_send_frames) = create_config(&engine, &args);
    let num_ports = config.num_ports();
    let top = engine.top().clone();
    let fabric: Rc<dyn Fabric<DataFrame>> = if args.routed {
        RoutedFabric::new_and_register(
            &engine,
            &clock,
            &top,
            "fabric",
            config.clone(),
            args.fabric_routing,
        )?
    } else {
        FunctionalFabric::new_and_register(&engine, &clock, &top, "fabric", config.clone())?
    };

    // By default enable all ports unless the user has constrained the generators
    let num_active_sources = match args.active_sources {
        Some(num_active_sources) => num_active_sources,
        None => config.num_ports(),
    };

    let (sources, sinks, total_expected_frames) = build_source_sinks(
        &mut engine,
        &clock,
        &config,
        args.traffic_pattern,
        args.frame_overhead_bytes,
        args.frame_payload_bytes,
        num_send_frames,
        args.seed,
        num_active_sources,
    );

    for i in 0..num_ports {
        connect_port!(sources[i], tx => fabric, ingress, i)?;
        connect_port!(fabric, egress, i => sinks[i], rx)?;
    }

    info!(top ; "Platform built and connected");

    let mut progress_bar = None;
    if args.progress {
        progress_bar = Some(ProgressBar::new(total_expected_frames as u64));
        let sinks = sinks.to_owned();
        start_frame_dump(
            &spawner,
            clock.clone(),
            args.progress_ticks,
            total_expected_frames,
            sinks,
            progress_bar.clone().unwrap(),
        );
    }

    if args.finish_tick != 0 {
        finish_at(&spawner, clock.clone(), args.finish_tick);
    }

    run_simulation!(engine);

    let mut total_sunk_frames = 0;
    for sink in &sinks {
        total_sunk_frames += sink.num_sunk();
    }

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
    let payload_bytes = total_sunk_frames * frame_payload_bytes;
    let (payload_value, payload_per_second) =
        compute_adjusted_value_and_rate(time_now_ns, payload_bytes);

    let total_bytes = payload_bytes + (total_sunk_frames * frame_overhead_bytes);
    let (total_value, total_per_second) = compute_adjusted_value_and_rate(time_now_ns, total_bytes);

    info!(top ; "Pass: Sent {total_sunk_frames} in {time_now_ns:.2}ns.");
    info!(top ; "Payload: {payload_value:.2} ({payload_per_second:.2}/s). Total: {total_value:.2} ({total_per_second:.2}/s).");
}
