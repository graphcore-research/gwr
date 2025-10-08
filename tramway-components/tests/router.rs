// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use tramway_components::connect_port;
use tramway_components::router::{DefaultAlgorithm, Router};
use tramway_components::sink::Sink;
use tramway_components::source::Source;
use tramway_engine::run_simulation;
use tramway_engine::test_helpers::start_test;

#[test]
fn router() {
    let mut engine = start_test(file!());

    const NUM_PUTS: usize = 50;

    let iter = Box::new((0..2).cycle().take(NUM_PUTS));
    let top = engine.top();
    let source = Source::new_and_register(&engine, top, "source", Some(iter)).unwrap();
    let router =
        Router::new_and_register(&engine, top, "router", 2, Box::new(DefaultAlgorithm {})).unwrap();
    let sink_a = Sink::new_and_register(&engine, top, "sink_a").unwrap();
    let sink_b = Sink::new_and_register(&engine, top, "sink_b").unwrap();

    connect_port!(source, tx => router, rx).unwrap();
    connect_port!(router, tx, 0 => sink_a, rx).unwrap();
    connect_port!(router, tx, 1 => sink_b, rx).unwrap();

    run_simulation!(engine);

    assert_eq!(sink_a.num_sunk(), NUM_PUTS / 2);
    assert_eq!(sink_b.num_sunk(), NUM_PUTS / 2);
}
