// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use steam_components::connect_port;
use steam_components::router::{DefaultRouter, Router};
use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_engine::run_simulation;
use steam_engine::test_helpers::start_test;

#[test]
fn router() {
    let mut engine = start_test(file!());

    const NUM_PUTS: usize = 50;

    let iter = Box::new((0..2).cycle().take(NUM_PUTS));
    let source = Source::new(engine.top(), "source", Some(iter));
    let router = Router::new(engine.top(), "router", 2, Box::new(DefaultRouter {}));
    let sink_a = Sink::new(engine.top(), "sink_a");
    let sink_b = Sink::new(engine.top(), "sink_b");

    connect_port!(source, tx => router, rx);
    connect_port!(router, tx, 0 => sink_a, rx);
    connect_port!(router, tx, 1 => sink_b, rx);

    run_simulation!(engine ; [source, router, sink_a, sink_b]);

    assert_eq!(sink_a.num_sunk(), NUM_PUTS / 2);
    assert_eq!(sink_b.num_sunk(), NUM_PUTS / 2);
}
