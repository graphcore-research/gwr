// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_engine::test_helpers::start_test;

#[test]
fn all_spawned() {
    let mut engine = start_test(file!());

    let source: Source<i32> = Source::new(engine.top(), "source", None);
    let sink = Sink::new(engine.top(), "sink");

    source.connect_port_tx(sink.port_rx());
    engine.spawn(async move { source.run().await });
    engine.spawn(async move { sink.run().await });
    engine.run().unwrap();
}
