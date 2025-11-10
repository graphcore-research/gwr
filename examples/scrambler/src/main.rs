// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! An example component which can switch the how its inputs are connected.
//!
//! See `lib.rs` for details.

use clap::Parser;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat};
use gwr_engine::engine::Engine;
use gwr_engine::run_simulation;
use gwr_engine::types::SimResult;
use gwr_track::entity::GetEntity;
use scrambler::Scrambler;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Example application using the Scrambler component")]
struct Cli {
    #[arg(long, short)]
    scramble: bool,
}

fn main() -> SimResult {
    let args = Cli::parse();

    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let top = engine.top();
    let scrambler = Scrambler::new_and_register(&engine, &clock, top, "scrambler", args.scramble)?;
    let source_a = Source::new_and_register(
        &engine,
        top,
        &format!("{}_{}", "source", "a"),
        option_box_repeat!(1 ; 1),
    )?;
    let source_b = Source::new_and_register(
        &engine,
        top,
        &format!("{}_{}", "source", "b"),
        option_box_repeat!(2 ; 2),
    )?;
    let sink_a = Sink::new_and_register(&engine, &clock, top, "sink_a")?;
    let sink_b = Sink::new_and_register(&engine, &clock, top, "sink_b")?;

    connect_port!(source_a, tx => scrambler, rx_a)?;
    connect_port!(source_b, tx => scrambler, rx_b)?;
    connect_port!(scrambler, tx_a => sink_a, rx)?;
    connect_port!(scrambler, tx_b => sink_b, rx)?;

    run_simulation!(engine);

    println!("Input order: {}, {}", sink_a.num_sunk(), sink_b.num_sunk());
    Ok(())
}
