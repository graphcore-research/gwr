// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use gwr_components::connect_port;
use gwr_components::router::{DefaultAlgorithm, Route, Router};
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::Routable;
use gwr_engine::types::{AccessType, DeviceId};

#[test]
fn router() {
    const NUM_PUTS: usize = 50;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let iter = Box::new((0..2).cycle().take(NUM_PUTS));
    let top = engine.top();
    let source = Source::new_and_register(&engine, top, "source", Some(iter));
    let router = Router::new_and_register(
        &engine,
        &clock,
        top,
        "router",
        2,
        Box::new(DefaultAlgorithm {}),
    );
    let sink_a = Sink::new_and_register(&engine, &clock, top, "sink_a");
    let sink_b = Sink::new_and_register(&engine, &clock, top, "sink_b");

    connect_port!(source, tx => router, rx).unwrap();
    connect_port!(router, tx, 0 => sink_a, rx).unwrap();
    connect_port!(router, tx, 1 => sink_b, rx).unwrap();

    run_simulation!(engine);

    assert_eq!(sink_a.num_sunk(), NUM_PUTS / 2);
    assert_eq!(sink_b.num_sunk(), NUM_PUTS / 2);
}

struct DeviceRoutable {
    dst_addr: u64,
    src_addr: u64,
    dst_device: DeviceId,
}

impl Routable for DeviceRoutable {
    fn dst_addr(&self) -> u64 {
        self.dst_addr
    }

    fn src_addr(&self) -> u64 {
        self.src_addr
    }

    fn dst_device(&self) -> DeviceId {
        self.dst_device
    }

    fn access_type(&self) -> AccessType {
        AccessType::ReadRequest
    }
}

#[test]
fn default_algorithm_routes_by_destination_device() {
    let object = DeviceRoutable {
        dst_addr: 0x1_0000_0000,
        src_addr: 0,
        dst_device: DeviceId(1),
    };

    assert_eq!(DefaultAlgorithm {}.route(&object).unwrap(), 1);
    assert_eq!(DefaultAlgorithm {}.route(&7_usize).unwrap(), 7);
}
