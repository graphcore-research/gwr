// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a device comprising a rectangular fabric.
//!
//! See `lib.rs` for details.
use std::collections::HashMap;
use std::rc::Rc;

use gwr_components::cli::{deserialize_optional_bytes, parse_bytes_string};
use gwr_components::connect_port;
use gwr_config::multi_source_config;
use gwr_engine::engine::Engine;
use gwr_engine::executor::Spawner;
use gwr_engine::time::clock::Clock;
use gwr_engine::time::compute_adjusted_value_and_rate;
use gwr_engine::types::SimError;
use gwr_engine::{run_simulation, sim_error};
use gwr_models::fabric::functional::FunctionalFabric;
use gwr_models::fabric::node::FabricRoutingAlgorithm;
use gwr_models::fabric::routed::RoutedFabric;
use gwr_models::fabric::{Fabric, FabricConfig, FabricGeometry, FabricPortConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_track::builder::{TrackerArgs, TrackerArgsPartial, setup_trackers};
use gwr_track::entity::Entity;
use gwr_track::{Track, error, info};
use indicatif::ProgressBar;
use sim_fabric::access_gen::TrafficPattern;
use sim_fabric::source_sink_builder::{Sinks, build_source_sinks};

/// Command-line arguments.
#[multi_source_config(default_conf_file = "sim-fabric.toml", conf_file_flag = "conf-file")]
#[derive(PartialEq)]
#[command(about = "Fabric evaluation application")]
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

    /// The number of columns in the fabric.
    #[arg(long, default_value_t = 4)]
    fabric_columns: Option<usize>,

    /// The number of rows in the fabric.
    #[arg(long, default_value_t = 3)]
    fabric_rows: Option<usize>,

    /// The number of ports at each node of the fabric.
    #[arg(long, default_value_t = 2)]
    fabric_ports_per_node: Option<usize>,

    /// The number of bytes to send from each source.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = parse_bytes_string("100KiB").unwrap())]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    bytes_to_send: Option<usize>,

    /// Set the number of bytes each fabric TX port can hold.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = parse_bytes_string("32KiB").unwrap())]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    tx_buffer_bytes: Option<usize>,

    /// Set the number of bytes each fabric RX port can hold.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = parse_bytes_string("32KiB").unwrap())]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    rx_buffer_bytes: Option<usize>,

    /// Set many bits per clock tick the fabric TX/RX ports move.
    #[arg(long, default_value_t = 128)]
    port_bits_per_tick: Option<usize>,

    /// Set the frame overhead (protocol) bytes.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = 8)]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    frame_overhead_bytes: Option<usize>,

    /// Set the default frame payload bytes.
    #[arg(long, value_parser = parse_bytes_string, default_value_t = 32)]
    #[serde(default, deserialize_with = "deserialize_optional_bytes")]
    frame_payload_bytes: Option<usize>,

    /// Set the clock ticks required to move one hop in the fabric.
    #[arg(long, default_value_t = 1)]
    ticks_per_hop: Option<usize>,

    /// An extra overhead for every frame passing through the fabric.
    #[arg(long, default_value_t = 1)]
    ticks_overhead: Option<usize>,

    /// What traffic pattern to use.
    #[arg(long, value_enum, default_value_t = TrafficPattern::default())]
    traffic_pattern: Option<TrafficPattern>,

    /// Number of active sources (chosen at random from possible sources).
    #[arg(long)]
    active_sources: Option<usize>,

    /// Seed for random number generator.
    #[arg(long, default_value_t = 1)]
    seed: Option<u64>,

    /// Whether or not to use the routed model
    #[arg(long, default_value_t = false)]
    routed: Option<bool>,

    /// Seed for random number generator.
    #[arg(long, value_enum, default_value_t = FabricRoutingAlgorithm::default())]
    fabric_routing: Option<FabricRoutingAlgorithm>,
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

fn create_config(engine: &Engine, args: &Cli) -> Result<(Rc<FabricConfig>, usize), SimError> {
    let num_fabric_ports = max_fabric_port_count(args)?;
    let config = FabricConfig::new(
        FabricGeometry {
            num_columns: args.fabric_columns.unwrap(),
            num_rows: args.fabric_rows.unwrap(),
            num_ports_per_node: args.fabric_ports_per_node.unwrap(),
            ports_per_node_limit: None,
        },
        FabricPortConfig {
            ticks_per_hop: args.ticks_per_hop.unwrap(),
            ticks_overhead: args.ticks_overhead.unwrap(),
            rx_buffer_bytes: args.rx_buffer_bytes.unwrap(),
            tx_buffer_bytes: args.tx_buffer_bytes.unwrap(),
            port_bits_per_tick: args.port_bits_per_tick.unwrap(),
        },
        identity_destination_port_map(num_fabric_ports),
    )?;
    let config = Rc::new(config);

    let num_payload_bytes_to_send = args.bytes_to_send.unwrap();

    // Size of max-sized frames
    let num_send_frames = num_payload_bytes_to_send / args.frame_payload_bytes.unwrap();

    let top = engine.top();
    info!(top ;
        "Fabric of {}x{}x{} sources, each sending {} frames ({} bytes) with buffers {}/{} bytes.",
        config.num_columns(),
        config.num_rows(),
        config.num_ports_per_node(),
        num_send_frames,
        args.bytes_to_send.unwrap(),
        args.rx_buffer_bytes.unwrap(),
        args.tx_buffer_bytes.unwrap(),
    );
    info!(top ; "Using traffic pattern {}. Random seed {}", args.traffic_pattern.unwrap(), args.seed.unwrap());

    Ok((config, num_send_frames))
}

fn max_fabric_port_count(args: &Cli) -> Result<usize, SimError> {
    args.fabric_columns
        .unwrap()
        .checked_mul(args.fabric_rows.unwrap())
        .and_then(|nodes| nodes.checked_mul(args.fabric_ports_per_node.unwrap()))
        .ok_or_else(|| SimError("maximum port count overflows".to_string()))
}

fn identity_destination_port_map(num_ports: usize) -> HashMap<u64, Vec<usize>> {
    (0..num_ports)
        .map(|port_idx| (port_idx as u64, vec![port_idx]))
        .collect()
}

fn main() -> Result<(), SimError> {
    let args = Cli::parse_all_sources();
    let tracker: Rc<dyn Track> = setup_trackers(&args.tracker.trackers_config()).unwrap();

    let mut engine = Engine::new(&tracker);
    let spawner = engine.spawner();
    let clock = engine.default_clock();

    let (config, num_send_frames) = create_config(&engine, &args)?;
    let num_ports = config.num_ports();
    let top = engine.top().clone();
    let fabric: Rc<dyn Fabric<MemoryAccess>> = if args.routed.unwrap() {
        RoutedFabric::new_and_register(
            &engine,
            &clock,
            &top,
            "fabric",
            config.clone(),
            args.fabric_routing.unwrap(),
        )?
    } else {
        FunctionalFabric::new_and_register(&engine, &clock, &top, "fabric", config.clone())?
    };

    // By default enable all ports unless the user has constrained the
    // generators
    let num_active_sources = match args.active_sources {
        Some(num_active_sources) => num_active_sources,
        None => config.num_ports(),
    };

    let (sources, sinks, total_expected_frames) = build_source_sinks(
        &mut engine,
        &clock,
        &config,
        args.traffic_pattern.unwrap(),
        args.frame_overhead_bytes.unwrap(),
        args.frame_payload_bytes.unwrap(),
        num_send_frames,
        args.seed.unwrap(),
        num_active_sources,
    );

    for i in 0..num_ports {
        connect_port!(sources[i], tx => fabric, ingress, i)?;
        connect_port!(fabric, egress, i => sinks[i], rx)?;
    }

    info!(top ; "Platform built and connected");

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
        args.frame_overhead_bytes.unwrap(),
        args.frame_payload_bytes.unwrap(),
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::{Cli, max_fabric_port_count};

    #[test]
    fn fabric_port_count_overflow_is_recoverable() {
        let matches = Cli::command().get_matches_from([
            "sim-fabric",
            "--fabric-columns",
            &usize::MAX.to_string(),
            "--fabric-rows",
            "2",
            "--fabric-ports-per-node",
            "1",
        ]);
        let args = Cli::parse_all_sources_with_matches(&matches);

        let error = max_fabric_port_count(&args).expect_err("overflow should return an error");

        assert_eq!(error.0, "maximum port count overflows");
    }
}
