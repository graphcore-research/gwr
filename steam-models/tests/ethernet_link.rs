// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::run_simulation;
use steam_engine::test_helpers::start_test;
use steam_engine::time::clock::Clock;
use steam_models::ethernet_frame::{EthernetFrame, PACKET_OVERHEAD_BYTES};
use steam_models::ethernet_link::{self, EthernetLink};

fn run_test(
    num_put_a: usize,
    num_put_b: usize,
    payload_bytes: usize,
) -> (Rc<Sink<EthernetFrame>>, Rc<Sink<EthernetFrame>>, Clock) {
    let mut engine = start_test(file!());

    let clock = engine.clock_ghz(1.0);
    let spawner = engine.spawner();
    let top = engine.top();

    let source_a = Source::new_and_register(&engine, top, "src_a", None).unwrap();
    let frame_a = EthernetFrame::new(&source_a.entity, payload_bytes);
    source_a.set_generator(option_box_repeat!(frame_a; num_put_a));

    let source_b = Source::new_and_register(&engine, top, "src_b", None).unwrap();
    let frame_b = EthernetFrame::new(&source_b.entity, payload_bytes);
    source_b.set_generator(option_box_repeat!(frame_b; num_put_b));

    let link =
        EthernetLink::new_and_register(&engine, top, "link", clock.clone(), spawner).unwrap();

    let sink_a = Sink::new_and_register(&engine, top, "sink_a").unwrap();
    let sink_b = Sink::new_and_register(&engine, top, "sink_b").unwrap();

    connect_port!(source_a, tx => link, rx_a).unwrap();
    connect_port!(source_b, tx => link, rx_b).unwrap();
    connect_port!(link, tx_a => sink_a, rx).unwrap();
    connect_port!(link, tx_b => sink_b, rx).unwrap();

    run_simulation!(engine);
    (sink_a, sink_b, clock)
}

#[test]
fn source_sink() {
    let num_puts_a = 100;
    let num_puts_b = 50;
    let (sink_a, sink_b, _) = run_test(num_puts_a, num_puts_b, 128);

    let num_sunk = sink_a.num_sunk();
    assert_eq!(num_sunk, num_puts_a);

    let num_sunk = sink_b.num_sunk();
    assert_eq!(num_sunk, num_puts_b);
}

#[test]
fn latency() {
    let num_puts_a = 1;
    let num_puts_b = 0;
    let (sink_a, sink_b, clock) = run_test(num_puts_a, num_puts_b, 128);

    let num_sunk = sink_a.num_sunk();
    assert_eq!(num_sunk, num_puts_a);

    let num_sunk = sink_b.num_sunk();
    assert_eq!(num_sunk, num_puts_b);

    let expected_time = ethernet_link::DELAY_TICKS as f64;
    assert_eq!(clock.time_now_ns(), expected_time);
}

#[test]
fn throughput() {
    let num_puts_a = 1000;
    let num_puts_b = 0;
    let payload_bytes: usize = 128;
    let (sink_a, sink_b, clock) = run_test(num_puts_a, num_puts_b, payload_bytes);

    let num_sunk = sink_a.num_sunk();
    assert_eq!(num_sunk, num_puts_a);

    let num_sunk = sink_b.num_sunk();
    assert_eq!(num_sunk, num_puts_b);

    let latency = ethernet_link::DELAY_TICKS as f64;
    let packet_bits = (payload_bytes + PACKET_OVERHEAD_BYTES) * 8;
    let packet_ticks = packet_bits.div_ceil(ethernet_link::BITS_PER_TICK);

    // Assume each tick is 1ns (1GHz clock) and that the throughput of the last
    // packet doesn't need to be counted
    let packets_time = (packet_ticks * (num_puts_a - 1)) as f64;
    assert_eq!(clock.time_now_ns(), (latency + packets_time));
}

#[test]
fn change_delay() {
    let mut engine = start_test(file!());

    const DELAY_TICKS: usize = 100;

    let clock = engine.clock_ghz(1.0);
    let spawner = engine.spawner();
    let top = engine.top();

    let source_a = Source::new_and_register(&engine, top, "src_a", None).unwrap();
    let etwr = EthernetFrame::new(&source_a.entity, 128);
    source_a.set_generator(option_box_repeat!(etwr; 1));

    let source_b = Source::new_and_register(&engine, top, "src_b", None).unwrap();

    let link =
        EthernetLink::new_and_register(&engine, top, "link", clock.clone(), spawner).unwrap();
    link.set_delay(DELAY_TICKS).unwrap();

    let sink_a = Sink::new_and_register(&engine, top, "sink_a").unwrap();
    let sink_b = Sink::new_and_register(&engine, top, "sink_b").unwrap();

    connect_port!(source_a, tx => link, rx_a).unwrap();
    connect_port!(source_b, tx => link, rx_b).unwrap();
    connect_port!(link, tx_a => sink_a, rx).unwrap();
    connect_port!(link, tx_b => sink_b, rx).unwrap();

    run_simulation!(engine);

    let num_sunk = sink_a.num_sunk();
    assert_eq!(num_sunk, 1);

    let expected_time = DELAY_TICKS as f64;
    assert_eq!(clock.time_now_ns(), expected_time);
}
