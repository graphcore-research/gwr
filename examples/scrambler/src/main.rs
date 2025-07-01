// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! This is an example using the scrambler component which can switch the two
//! inputs passing through it. The main purpose of this component is to show
//! how the user can register a vector of subcomponents.
//! See [Scrambler] (crate::examples::scrambler::Scrambler)
//!
//! For latest usage run:
//! ```bash
//! cargo run --bin scrambler -- --help
//! ```
//!
//! # Examples
//!
//! Get the two inputs in the same order:
//! ```bash
//! $ cargo run --bin scrambler
//! Input order: 1, 2
//! ```
//!
//! Switch the two inputs:
//! ```bash
//! $ cargo run --bin scrambler -- -s
//! Input order: 2, 1
//! ```

use clap::Parser;
use scrambler::Scrambler;
use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::engine::Engine;
use steam_engine::run_simulation;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Example application using the Scrambler component")]
struct Cli {
    #[arg(long, short)]
    scramble: bool,
}

fn main() {
    let args = Cli::parse();

    let mut engine = Engine::default();
    let spawner = engine.spawner();
    let top = engine.top();
    let scrambler = Scrambler::new_and_register(&engine, top, "scrambler", spawner, args.scramble);
    let source_a = Source::new_and_register(
        &engine,
        top,
        &format!("{}_{}", "source", "a"),
        option_box_repeat!(1 ; 1),
    );
    let source_b = Source::new_and_register(
        &engine,
        top,
        &format!("{}_{}", "source", "b"),
        option_box_repeat!(2 ; 2),
    );
    let sink_a = Sink::new_and_register(&engine, top, "sink_a");
    let sink_b = Sink::new_and_register(&engine, top, "sink_b");

    connect_port!(source_a, tx => scrambler, rx_a);
    connect_port!(source_b, tx => scrambler, rx_b);
    connect_port!(scrambler, tx_a => sink_a, rx);
    connect_port!(scrambler, tx_b => sink_b, rx);

    run_simulation!(engine);

    println!("Input order: {}, {}", sink_a.num_sunk(), sink_b.num_sunk());
}
