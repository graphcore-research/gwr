// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat};
use gwr_engine::engine::Engine;

fn main() {
    let num_puts = 10;
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let top = engine.top();
    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(0x123 ; num_puts))
            .unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();
    connect_port!(source, tx => sink, invalid_rx).unwrap();
}
