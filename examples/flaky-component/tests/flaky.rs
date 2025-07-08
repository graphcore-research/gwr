// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use flaky_component::Flaky;
use steam_components::sink::Sink;
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
    let top = engine.top();
    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(0x123 ; num_puts))
            .unwrap();
    let flaky = Flaky::new_and_register(&engine, top, "flaky", drop, seed).unwrap();
    let sink = Sink::new_and_register(&engine, top, "sink").unwrap();

    connect_port!(source, tx => flaky, rx).unwrap();
    connect_port!(flaky, tx => sink, rx).unwrap();

    run_simulation!(engine);

    // We requested 50%, but it might be slightly more
    assert!(sink.num_sunk() < 55);
}
