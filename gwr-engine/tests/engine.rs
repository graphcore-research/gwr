// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;

#[test]
fn all_spawned() {
    let mut engine = start_test(file!());

    let top = engine.top();
    let source: Rc<Source<i32>> = Source::new_and_register(&engine, top, "source", None).unwrap();
    let sink = Sink::new_and_register(&engine, top, "sink").unwrap();

    source.connect_port_tx(sink.port_rx()).unwrap();
    run_simulation!(engine);
}
