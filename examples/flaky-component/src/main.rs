// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This is an example using the flaky component which randomly drops data being
//! passed through it.
//!
//! For latest usage run:
//! ```bash
//! cargo run --bin flaky-component -- --help
//! ```
//!
//! # Example
//!
//! Send 10000 packets and drop 50% of them:
//! ```bash
//! $ cargo run --bin flaky-component -- --seed 1 --drop 0.5 --num-packets 10000
//! Sink received 4934/10000
//! ```

use clap::Parser;
use flaky_component::Flaky;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat};
use gwr_engine::engine::Engine;
use gwr_engine::run_simulation;
use gwr_engine::types::SimResult;

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
}

fn main() -> SimResult {
    let args = Cli::parse();

    let mut engine = Engine::default();

    let num_puts = args.num_packets;

    let top = engine.top();
    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(0x123 ; num_puts))?;
    let flaky = Flaky::new_and_register(&engine, top, "flaky", args.drop, args.seed)?;
    let sink = Sink::new_and_register(&engine, top, "sink")?;

    connect_port!(source, tx => flaky, rx)?;
    connect_port!(flaky, tx => sink, rx)?;

    run_simulation!(engine);

    println!("Sink received {}/{}", sink.num_sunk(), num_puts);
    Ok(())
}
