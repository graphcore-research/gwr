// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
//
//! Simulate a device comprising ring nodes.
//!
//! The model is constructed with as many ring nodes as specified by
//! the user. Each ring node will receive Ethernet Frames from a source
//! that should be routed all the way around the ring and end up at
//! the node effectively to its left.
//!
//! Limiters and flow control pipes are added to model actual hardware
//! limitations.
//!
//! The ring node contains an arbiter used to decide which packet to
//! grant next; the next ring packet or a new packet from the source.
//! The ring priority can be configured to demonstrate that incorrect
//! priority will lead to deadlock.
//!
//! # Examples
//!
//! Running a ring node that will lock up:
//! ```rust
//! ./target/release/sim-ring --kb-to-send 1024 --stdout
//! ```
//!
//! But with increased ring priority the same model will pass:
//! ```rust
//! ./target/release/sim-ring --kb-to-send 1024 --ring-priority 10 --stdout
//! ```
//!
//! # Diagram
//!
//! ```text
//!  /------------------------------------------------------------\
//!  |                                                            |
//!  |  +--------+                             +--------+         |
//!  |  | Source |                             | Source |         |
//!  |  +--------+                             +--------+         |
//!  |     |                                      |               |
//!  |     v                                      v               |
//!  |  +---------+                            +---------+        |
//!  |  | Limiter |                            | Limiter |        |
//!  |  +---------+                            +---------+        |
//!  |     |                                      |               |
//!  |     v                                      v               |
//!  |  +--------+                             +--------+         |
//!  |  | FcPipe |                             | FcPipe |         |
//!  |  +--------+                             +--------+         |
//!  |     |                                      |               |
//!  |     v                                      v               |
//!  |  +----------+  +---------+  +--------+  +----------+       |
//!  \->| RingNode |->| Limiter |->| FcPipe |->| RingNode | ...  -/
//!     +----------+  +---------+  +--------+  +----------+
//!        |                                      |
//!        v                                      v
//!     +---------+                            +---------+
//!     | Limiter |                            | Limiter |
//!     +---------+                            +---------+
//!        |                                      |
//!        v                                      v
//!     +------+                               +------+
//!     | Sink |                               | Sink |
//!     +------+                               +------+
//! ```

use clap::Parser;
use indicatif::ProgressBar;
use sim_ring::ring_builder::{
    Config, Sinks, build_limiters, build_pipes, build_ring_nodes, build_source_sinks,
};
use sim_ring::tracker_builder::setup_trackers;
use steam_components::connect_port;
use steam_engine::engine::Engine;
use steam_engine::executor::Spawner;
use steam_engine::time::clock::Clock;
use steam_engine::{run_simulation, sim_error};
use steam_models::ethernet_frame::PACKET_OVERHEAD_BYTES;
use steam_track::{error, info};

// Define the standard Ethernet data rate
const ETHERNET_GBPS: usize = 100;

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

    /// Enable logging to binary file used by `steam-spotter`.
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

    /// The number of nodes in the ring.
    #[arg(long, default_value = "8")]
    ring_size: usize,

    /// The number of kB to send from each source.
    #[arg(long, default_value = "100")]
    kb_to_send: usize,

    /// The priority of ring traffic over local traffic in the arbiter.
    #[arg(long, default_value = "1")]
    ring_priority: usize,

    /// Override the default number of kB in the Tx buffer.
    #[arg(long, default_value = "32")]
    tx_buffer_kb: usize,

    /// Override the default number of kB in the Rx buffer.
    #[arg(long, default_value = "32")]
    rx_buffer_kb: usize,

    /// Override the default packet payload bytes.
    #[arg(long, default_value = "256")]
    packet_payload_bytes: usize,
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

type SimRingError = &'static str;

fn main() -> Result<(), SimRingError> {
    let args = Cli::parse();

    let tracker = setup_trackers(
        args.stdout,
        args.stdout_level,
        args.stdout_filter_regex.as_str(),
        args.binary,
        args.binary_level,
        &args.binary_filter_regex,
        &args.binary_file,
    );

    let mut engine = Engine::new(&tracker);
    let spawner = engine.spawner();
    let clock = engine.default_clock();

    let tx_buffer_bytes = args.tx_buffer_kb * 1024;
    let rx_buffer_bytes = args.rx_buffer_kb * 1024;
    let num_payload_bytes_to_send = args.kb_to_send * 1024;

    // Size of max-sized EthernetFrame packets
    let packet_bytes = args.packet_payload_bytes + PACKET_OVERHEAD_BYTES;

    let config = Config {
        ring_size: args.ring_size,
        ring_priority: args.ring_priority,
        rx_buffer_frames: rx_buffer_bytes / packet_bytes,
        tx_buffer_frames: tx_buffer_bytes / packet_bytes,
        packet_payload_bytes: args.packet_payload_bytes,
        num_send_packets: num_payload_bytes_to_send / args.packet_payload_bytes,
    };

    let top = engine.top().clone();
    info!(top ;
        "Ring of {} sources, priority {}, each sending {} packets ({}kB) with buffers {}/{} packets.",
        config.ring_size,
        config.ring_priority,
        config.num_send_packets,
        args.kb_to_send,
        config.rx_buffer_frames,
        config.tx_buffer_frames
    );

    let ring_nodes = build_ring_nodes(&mut engine, &config);
    let (sources, sinks) = build_source_sinks(&mut engine, &config);
    let (ingress_pipes, ring_pipes) = build_pipes(&mut engine, &config);
    let (source_limiters, ring_limiters, sink_limiters) =
        build_limiters(&mut engine, &config, ETHERNET_GBPS);

    for i in 0..config.ring_size {
        let right = (i + 1) % config.ring_size;

        // Connect the sources to the ring using a rater limiter and flow controlled
        // pipeline.
        connect_port!(sources[i], tx => source_limiters[i], rx);
        connect_port!(source_limiters[i], tx => ingress_pipes[i], rx);
        connect_port!(ingress_pipes[i], tx => ring_nodes[i], io_rx);

        // Connect the ring together using a rate limiter and a flow controlled
        // pipeline.
        connect_port!(ring_nodes[i], ring_tx => ring_limiters[i], rx);
        connect_port!(ring_limiters[i], tx => ring_pipes[i], rx);
        connect_port!(ring_pipes[i], tx => ring_nodes[right], ring_rx);

        // Connect the ring to the sinks using a rate limiter.
        connect_port!(ring_nodes[i], io_tx => sink_limiters[i], rx);
        connect_port!(sink_limiters[i], tx => sinks[i], rx);
    }

    info!(top ; "Platform built and connected");

    if args.progress {
        let total_expected_packets = config.num_send_packets * config.ring_size;
        let sinks = sinks.to_owned();
        start_packet_dump(
            &spawner.clone(),
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

    for sink in &sinks {
        if sink.num_sunk() != config.num_send_packets {
            error!(top ; "{}/{} packets received", sink.num_sunk(), config.num_send_packets);
            error!(top ; "Deadlock detected at {:.2}ns", clock.time_now_ns());

            tracker.shutdown();
            return Err("Deadlock");
        }
    }
    info!(top ; "Pass ({:.2}ns)", clock.time_now_ns());
    Ok(())
}
