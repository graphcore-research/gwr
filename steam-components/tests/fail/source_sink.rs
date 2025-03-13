// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use steam_components::sinks::Sink;
use steam_components::source::Source;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::engine::Engine;

fn main() {
    let num_puts = 10;
    let engine = Engine::default();
    let source = Source::new(engine.top(), "source", option_box_repeat!(0x123 ; num_puts));
    let sink = Sink::new(engine.top(), "sink");
    connect_port!(source, tx => sink, invalid_rx);
}
