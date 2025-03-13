// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use flaky_component::Flaky;
use steam_components::sinks::Sink;
use steam_components::source::Source;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::engine::Engine;
use steam_engine::run_simulation;
use steam_engine::test_helpers::create_tracker;

/// Command-line arguments.
#[test]
fn drop_precent() {
    let tracker = create_tracker(file!());
    let mut engine = Engine::new(&tracker);
    let num_puts = 100;

    let drop = 0.5;
    let seed = 1;
    let source = Source::new(engine.top(), "source", option_box_repeat!(0x123 ; num_puts));
    let mut flaky = Flaky::new(engine.top(), "flaky", drop, seed);
    let sink = Sink::new(engine.top(), "sink");

    connect_port!(source, tx => flaky, rx);
    connect_port!(flaky, tx => sink, rx);

    run_simulation!(engine ; [source, flaky, sink]);

    // We requested 50%, but it might be slightly more
    assert!(sink.num_sunk() < 55);
}
