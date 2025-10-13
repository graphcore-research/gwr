// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a device comprising a rectangular fabric.
//!
//! The model is constructed the specified fabric and traffic generators
//! and sinks connected to all of the fabric ports.
//!
//! The traffic generators can be configured to send different traffic
//! patterns in order to evaluate the performance of the fabric.
//!
//! # Examples
//!
//! Running a basic all-to-all simulation
//! ```rust
//! cargo run --bin sim-fabric --release -- --kb-to-send 1024 --stdout
//! ```

// TODO: wire up the active_sources

use std::rc::Rc;

use clap::Parser;
use indicatif::ProgressBar;
use sim_fabric::packet_gen::TrafficPattern;
use sim_fabric::source_sink_builder::{Sinks, build_source_sinks};
use tramway_components::connect_port;
use tramway_engine::engine::Engine;
use tramway_engine::executor::Spawner;
use tramway_engine::time::clock::Clock;
use tramway_engine::types::SimError;
use tramway_engine::{run_simulation, sim_error};
use tramway_models::fabric::FabricConfig;
use tramway_models::fabric::functional::Fabric;
use tramway_track::builder::setup_all_trackers;
use tramway_track::{error, info};

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Ring deadlock test")]
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

    /// Enable logging to binary file used by `tramway-spotter`.
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

    /// Enable logging to Perfetto file used by `tramway-spotter`.
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
    /// defined by `progress_ticks`). NOTE: with the progress bar enabled
    /// the simulation will not finish if it deadlocks.
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

    /// The number of kB to send from each source.
    #[arg(long, default_value = "100")]
    kb_to_send: usize,

    /// Set the number of packets each fabric TX port can hold.
    #[arg(long, default_value = "32")]
    tx_buffer_entries: usize,

    /// Set the number of packets each fabric RX port can hold.
    #[arg(long, default_value = "32")]
    rx_buffer_entries: usize,

    /// Set many bits per clock tick the fabric TX/RX ports move.
    #[arg(long, default_value = "128")]
    port_bits_per_tick: usize,

    /// Set the default packet payload bytes.
    #[arg(long, default_value = "256")]
    packet_payload_bytes: usize,

    /// Set the clock ticks required to move one hop in the fabric.
    #[arg(long, default_value = "1")]
    ticks_per_hop: usize,

    /// An extra overhead for every packet passing through the fabric.
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
}

/// Install an event to terminate the simulation at the clock tick defined.
fn finish_at(spawner: &Spawner, clock: Clock, run_ticks: usize) {
    spawner.spawn(async move {
        clock.wait_ticks(run_ticks as u64).await;
        sim_error!("Finish")
    });
}

/// Spawn a background task to display regular updates of the total number of
/// packets received so far.
fn start_packet_dump(
    spawner: &Spawner,
    clock: Clock,
    progress_ticks: usize,
    total_expected_packets: usize,
    sinks: Sinks,
) {
    spawner.spawn(async move {
        let pb = ProgressBar::new(total_expected_packets as u64);
        let mut seen_packets = 0;
        loop {
            // Use the `background` wait to indicate that the simulation can end if this is
            // the only task still active.
            clock.wait_ticks_or_exit(progress_ticks as u64).await;
            let num_packets: usize = sinks.iter().map(|s| s.num_sunk()).sum();
            pb.inc((num_packets - seen_packets) as u64);
            seen_packets = num_packets;
            if num_packets == total_expected_packets {
                break;
            }
        }
        Ok(())
    });
}

fn main() -> Result<(), SimError> {
    let args = Cli::parse();

    let tracker = setup_all_trackers(
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
    .unwrap();

    let mut engine = Engine::new(&tracker);
    let spawner = engine.spawner();
    let clock = engine.default_clock();

    let config = FabricConfig::new(
        args.fabric_columns,
        args.fabric_rows,
        args.fabric_ports_per_node,
        args.ticks_per_hop,
        args.ticks_overhead,
        args.rx_buffer_entries,
        args.tx_buffer_entries,
        args.port_bits_per_tick,
    );
    let config = Rc::new(config);
    let num_ports = config.num_ports();

    let num_payload_bytes_to_send = args.kb_to_send * 1024;

    // Size of max-sized EthernetFrame packets
    let num_send_packets = num_payload_bytes_to_send / args.packet_payload_bytes;

    let top = engine.top().clone();
    info!(top ;
        "Fabric of {}x{}x{} sources, each sending {} packets ({}kB) with buffers {}/{} packets.",
        config.num_columns(),
        config.num_rows(),
        config.num_ports_per_node(),
        num_send_packets,
        args.kb_to_send,
        args.rx_buffer_entries,
        args.tx_buffer_entries,
    );
    info!(top ; "Using traffic pattern {}. Random seed {}", args.traffic_pattern, args.seed);

    let fabric = Fabric::new_and_register(
        &engine,
        &top,
        "fabric",
        clock.clone(),
        spawner.clone(),
        config.clone(),
    )?;
    let (sources, sinks) = build_source_sinks(
        &mut engine,
        &config,
        args.traffic_pattern,
        args.packet_payload_bytes,
        num_send_packets,
        args.seed,
    );

    for i in 0..num_ports {
        connect_port!(sources[i], tx => fabric, rx, i)?;
        connect_port!(fabric, tx, i => sinks[i], rx)?;
    }

    info!(top ; "Platform built and connected");

    if args.progress {
        let total_expected_packets = num_send_packets * num_ports;
        let sinks = sinks.to_owned();
        start_packet_dump(
            &spawner,
            clock.clone(),
            args.progress_ticks,
            total_expected_packets,
            sinks,
        );
    }

    if args.finish_tick != 0 {
        finish_at(&spawner, clock.clone(), args.finish_tick);
    }

    run_simulation!(engine);

    let mut total_sunk = 0;
    for sink in &sinks {
        total_sunk += sink.num_sunk();
    }

    let total_expected = num_send_packets * config.num_ports();
    if total_sunk != total_expected {
        error!(top ; "{}/{} packets received", total_sunk, total_expected);
        error!(top ; "Deadlock detected at {:.2}ns", clock.time_now_ns());

        tracker.shutdown();
        return sim_error!("Deadlock");
    }

    info!(top ; "Pass ({:.2}ns)", clock.time_now_ns());
    Ok(())
}
