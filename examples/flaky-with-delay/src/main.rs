// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This is an example using the flaky component which randomly drops data being
//! passed through it.
//!
//! For latest usage run:
//! ```bash
//! cargo run --bin flaky-with-delay -- --help
//! ```
//!
//! # Example
//!
//! Send 10000 packets and drop 50% of them:
//! ```bash
//! $ cargo run --bin flaky-with-delay -- --seed 1 --drop 0.5 --num-packets 10000
//! Sink received 4934/10000
//! ```

use std::process::exit;

use flaky_with_delay::{Config, Flaky};
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat};
use gwr_config::multi_source_config;
use gwr_engine::engine::Engine;
use gwr_engine::run_simulation;
use gwr_engine::types::SimResult;

/// Command-line arguments.
#[multi_source_config]
#[command(about = "Example application using the Flaky component with delay")]
struct Cli {
    /// Set the random seed
    #[arg(long, default_value_t = 123)]
    seed: Option<u64>,

    /// The ratio of data to be dropped (should be in the range [0, 1])
    #[arg(long, default_value_t = 0.2)]
    drop: Option<f64>,

    /// The number of packets to send through the component
    #[arg(long, default_value_t = 100)]
    num_packets: Option<usize>,

    /// The delay through the flaky component
    #[arg(long, default_value_t = 10)]
    delay: Option<usize>,
}

fn main() -> SimResult {
    let args = Cli::parse_all_sources();

    let mut engine = Engine::default();
    let clock = engine.default_clock();

    let num_puts = args.num_packets.unwrap();

    let top = engine.top();
    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(0x123 ; num_puts));

    let drop = args.drop.unwrap();
    if !(0.0..=1.0).contains(&drop) {
        println!("ERROR: --drop ratio outside valid range [0, 1]");
        exit(1);
    }
    let config = Config::new(drop, args.seed.unwrap(), args.delay.unwrap());
    let flaky = Flaky::new_and_register(&engine, &clock, top, "flaky", &config)?;
    let sink = Sink::new_and_register(&engine, &clock, top, "sink");

    connect_port!(source, tx => flaky, rx)?;
    connect_port!(flaky, tx => sink, rx)?;

    run_simulation!(engine);

    println!("Sink received {}/{}", sink.num_sunk(), num_puts);
    Ok(())
}
