// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_components::arbiter::policy::WeightedRoundRobin;
use gwr_components::router::Route;
use gwr_components::sink::Sink;
use gwr_components::{connect_port, rc_limiter};
use gwr_engine::engine::Engine;
use gwr_engine::port::OutPort;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Routable;
use gwr_engine::types::SimError;
use gwr_models::ethernet_frame::{EthernetFrame, SRC_MAC_BYTES, mac_to_u64};
use gwr_models::ring_node::{IO_INDEX, RING_INDEX, RingConfig, RingNode};

struct TestAlgorithm(u64);

impl<T> Route<T> for TestAlgorithm
where
    T: Routable,
{
    fn route(&self, obj: &T) -> Result<usize, SimError> {
        // If the dest matches then exit via IO port, otherwise use ring port
        let dest = obj.destination();
        Ok(if self.0 == dest { IO_INDEX } else { RING_INDEX })
    }
}

fn run_test(
    mut engine: Engine,
    mut ring_frames: Vec<EthernetFrame>,
    mut io_frames: Vec<EthernetFrame>,
    weights: Vec<usize>,
    route_fn: Box<dyn Route<EthernetFrame>>,
) -> (Rc<Sink<EthernetFrame>>, Rc<Sink<EthernetFrame>>, Clock) {
    let rx_buffer_bytes = 1024;
    let tx_buffer_bytes = 1024;

    let clock = engine.clock_ghz(1.0);
    let top = engine.top();

    let limiter_128b_per_tick = rc_limiter!(&clock, 128);

    let config = RingConfig::new(rx_buffer_bytes, tx_buffer_bytes, limiter_128b_per_tick);
    let ring_node = RingNode::new_and_register(
        &engine,
        &clock,
        top,
        "dut",
        &config,
        route_fn,
        Box::new(WeightedRoundRobin::new(weights, 2).unwrap()),
    )
    .unwrap();

    {
        let mut ring_tx = OutPort::new(engine.top(), "ring_tx");
        ring_tx.connect(ring_node.port_ring_rx()).unwrap();
        engine.spawn(async move {
            for frame in ring_frames.drain(..) {
                ring_tx.put(frame)?.await;
            }
            Ok(())
        });
    }

    {
        let mut io_tx = OutPort::new(engine.top(), "io_tx");
        io_tx.connect(ring_node.port_io_rx()).unwrap();
        engine.spawn(async move {
            for frame in io_frames.drain(..) {
                io_tx.put(frame)?.await;
            }
            Ok(())
        });
    }

    let io_sink = Sink::new_and_register(&engine, &clock, top, "io_sink").unwrap();
    let ring_sink = Sink::new_and_register(&engine, &clock, top, "ring_sink").unwrap();

    connect_port!(ring_node, io_tx => io_sink, rx).unwrap();
    connect_port!(ring_node, ring_tx => ring_sink, rx).unwrap();

    run_simulation!(engine);
    (ring_sink, io_sink, clock)
}

#[test]
fn ring_to_io() {
    let num_ring_tx = 50;
    let ring_tx_payload_size_bytes = 256;
    let num_io_tx = 20;
    let io_tx_payload_size_bytes = 128;
    let weights = vec![1, 1];

    let io_dest = [0, 1, 2, 3, 4, 5];
    // Ensure that the frames coming into the IO port are destined to a different
    // destination
    let mut ring_dest = io_dest;
    ring_dest[0] = !ring_dest[0];

    let engine = start_test(file!());
    let mut ring_frames = Vec::with_capacity(num_ring_tx);
    for i in 0..num_ring_tx {
        let frame = EthernetFrame::new(engine.top(), ring_tx_payload_size_bytes)
            .set_dest(io_dest)
            .set_src([i as u8; SRC_MAC_BYTES]);
        ring_frames.push(frame);
    }

    let mut io_frames = Vec::with_capacity(num_io_tx);
    for i in 0..num_io_tx {
        let frame = EthernetFrame::new(engine.top(), io_tx_payload_size_bytes)
            .set_dest(ring_dest)
            .set_src([!i as u8; SRC_MAC_BYTES]);
        io_frames.push(frame);
    }

    let router = Box::new(TestAlgorithm(mac_to_u64(&io_dest)));
    let (ring_sink, io_sink, _) = run_test(engine, ring_frames, io_frames, weights, router);

    // The ring packets should have come from the IO TX
    let num_sunk = ring_sink.num_sunk();
    assert_eq!(num_sunk, num_io_tx);

    // The IO packets should have come from the ring TX
    let num_sunk = io_sink.num_sunk();
    assert_eq!(num_sunk, num_ring_tx);
}

#[test]
fn all_to_ring() {
    // Test routing all packets to the IO port
    let num_ring_tx = 5;
    let ring_tx_payload_size_bytes = 256;
    let num_io_tx = 2;
    let io_tx_payload_size_bytes = 128;
    let weights = vec![1, 1];

    let io_dest = [0, 1, 2, 3, 4, 5];
    // Ensure that the frames coming into the IO port are destined to a different
    // destination
    let mut ring_dest = io_dest;
    ring_dest[0] = !ring_dest[0];

    let engine = start_test(file!());
    let mut ring_frames = Vec::with_capacity(num_ring_tx);
    for i in 0..num_ring_tx {
        let frame = EthernetFrame::new(engine.top(), ring_tx_payload_size_bytes)
            .set_dest(ring_dest)
            .set_src([i as u8; SRC_MAC_BYTES]);
        ring_frames.push(frame);
    }

    let mut io_frames = Vec::with_capacity(num_io_tx);
    for i in 0..num_io_tx {
        let frame = EthernetFrame::new(engine.top(), io_tx_payload_size_bytes)
            .set_dest(ring_dest)
            .set_src([!i as u8; SRC_MAC_BYTES]);
        io_frames.push(frame);
    }

    let router = Box::new(TestAlgorithm(mac_to_u64(&io_dest)));
    let (ring_sink, io_sink, _) = run_test(engine, ring_frames, io_frames, weights, router);

    // All packets should be sent along the ring
    let num_sunk = ring_sink.num_sunk();
    assert_eq!(num_sunk, num_ring_tx + num_io_tx);

    // Nothing should be seen on the IO port
    let num_sunk = io_sink.num_sunk();
    assert_eq!(num_sunk, 0);
}
