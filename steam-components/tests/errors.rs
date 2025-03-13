// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use steam_components::sinks::Sink;
use steam_components::source::Source;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::test_helpers::start_test;

#[test]
#[should_panic(expected = "top::source: tx already connected")]
fn connect_twice() {
    let engine = start_test(file!());

    let source = Source::new(engine.top(), "source", option_box_repeat!(1 ; 1));

    let sink1 = Sink::new(engine.top(), "sink1");
    let sink2 = Sink::new(engine.top(), "sink2");

    connect_port!(source, tx => sink1, rx);
    connect_port!(source, tx => sink2, rx);
}
