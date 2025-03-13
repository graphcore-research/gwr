// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This is an example using the flaky component which randomly drops data being
//! passed through it.
//!
//! For latest usage run:
//! ```bash
//! cargo run --bin flaky-with_delay -- --help
//! ```
//!
//! # Example
//!
//! Send 10000 packets and drop 50% of them:
//! ```bash
//! $ cargo run --bin flaky-with_delay -- --seed 1 --drop 0.5 --num-packets 10000
//! Sink received 4934/10000
//! ```

use clap::Parser;
use flaky_with_delay::Flaky;
use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::engine::Engine;
use steam_engine::run_simulation;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Example application using the Flaky component")]
struct Cli {
    /// Set the random seed
    #[arg(long, default_value = "123")]
    seed: u64,

    /// The ratio of data to be dropped (should be in the range [0, 1])
    #[arg(long, default_value = "0.2")]
    drop: f64,

    /// The ratio of data to be dropped (should be in the range [0, 1])
    #[arg(long, default_value = "100")]
    num_packets: usize,

    /// The delay through the flaky component
    #[arg(long, default_value = "10")]
    delay: usize,
}

fn main() {
    let args = Cli::parse();

    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let spawner = engine.spawner();

    let num_puts = args.num_packets;

    let source = Source::new(engine.top(), "source", option_box_repeat!(0x123 ; num_puts));
    let mut flaky = Flaky::new(
        engine.top(),
        "flaky",
        clock,
        spawner,
        args.drop,
        args.seed,
        args.delay,
    );
    let sink = Sink::new(engine.top(), "sink");

    connect_port!(source, tx => flaky, rx);
    connect_port!(flaky, tx => sink, rx);

    run_simulation!(engine ; [source, flaky, sink]);

    println!("Sink received {}/{}", sink.num_sunk(), num_puts);
}
