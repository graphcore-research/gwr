// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use tramway_components::sink::Sink;
use tramway_components::source::Source;
use tramway_components::{connect_port, option_box_repeat};
use tramway_engine::engine::Engine;

fn main() {
    let num_puts = 10;
    let engine = Engine::default();
    let top = engine.top();
    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(0x123 ; num_puts))
            .unwrap();
    let sink = Sink::new_and_register(&engine, top, "sink").unwrap();
    connect_port!(source, tx => sink, invalid_rx).unwrap();
}
