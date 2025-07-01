// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::test_helpers::start_test;

#[test]
#[should_panic(expected = "top::source::tx already connected")]
fn connect_outport_twice() {
    let engine = start_test(file!());

    let top = engine.top();
    let source = Source::new_and_register(&engine, top, "source", option_box_repeat!(1 ; 1));

    let sink1 = Sink::new_and_register(&engine, top, "sink1");
    let sink2 = Sink::new_and_register(&engine, top, "sink2");

    connect_port!(source, tx => sink1, rx);
    connect_port!(source, tx => sink2, rx);
}

#[test]
#[should_panic(expected = "top::sink::rx already connected")]
fn connect_inport_twice() {
    let engine = start_test(file!());

    let top = engine.top();
    let source1 = Source::new_and_register(&engine, top, "source1", option_box_repeat!(1 ; 1));
    let source2 = Source::new_and_register(&engine, top, "source2", option_box_repeat!(1 ; 1));

    let sink = Sink::new_and_register(&engine, top, "sink");

    connect_port!(source1, tx => sink, rx);
    connect_port!(source2, tx => sink, rx);
}
