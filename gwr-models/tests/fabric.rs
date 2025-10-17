// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_components::connect_port;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_engine::engine::Engine;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::TotalBytes;
use gwr_models::ethernet_frame::{EthernetFrame, SRC_MAC_BYTES, u64_to_mac};
use gwr_models::fabric::FabricConfig;
use gwr_models::fabric::functional::Fabric;

trait ToDest {
    fn to_dest(&self, source_index: usize, frame_index: usize) -> [u8; SRC_MAC_BYTES];
}

struct FixedDest(u64);

impl ToDest for FixedDest {
    fn to_dest(&self, _source_index: usize, _frame_index: usize) -> [u8; SRC_MAC_BYTES] {
        u64_to_mac(self.0)
    }
}

struct ToNext {
    num_ports: usize,
}

impl ToDest for ToNext {
    fn to_dest(&self, source_index: usize, _frame_index: usize) -> [u8; SRC_MAC_BYTES] {
        let dest = (source_index + 1) % self.num_ports;
        u64_to_mac(dest as u64)
    }
}

// Create series of frames all destined to one egress port
fn build_frames_for_one_dest(
    engine: &Engine,
    source_index: usize,
    to_dest: &impl ToDest,
    num_frames: usize,
    payload_bytes: usize,
) -> Vec<EthernetFrame> {
    let mut source_mac = u64_to_mac(source_index as u64);
    let mut frames = Vec::with_capacity(num_frames);
    for i in 0..num_frames {
        // Aid debugging by making packets easier to track
        source_mac[5] = i as u8;

        let frame = EthernetFrame::new(engine.top(), payload_bytes)
            .set_dest(to_dest.to_dest(source_index, i))
            .set_src(source_mac);
        frames.push(frame);
    }
    frames
}

fn run_test(
    config: Rc<FabricConfig>,
    to_dest: &impl ToDest,
    num_frames: usize,
    payload_bytes: usize,
) -> Vec<Rc<Sink<EthernetFrame>>> {
    let mut engine = start_test(file!());

    let clock = engine.clock_ghz(1.0);
    let spawner = engine.spawner();
    let top = engine.top();

    let fabric =
        Fabric::new_and_register(&engine, top, "fabric", clock, spawner, config.clone()).unwrap();

    let num_ports = config.num_ports();
    let mut sources = Vec::with_capacity(num_ports);
    let mut sinks = Vec::with_capacity(num_ports);

    for i in 0..num_ports {
        let source =
            Source::new_and_register(&engine, top, format!("source{i}").as_str(), None).unwrap();
        source.set_generator(Some(Box::new(
            build_frames_for_one_dest(&engine, i, to_dest, num_frames, payload_bytes).into_iter(),
        )));
        connect_port!(source, tx => fabric, rx, i).unwrap();
        sources.push(source);

        let sink = Sink::new_and_register(&engine, top, format!("sink{i}").as_str()).unwrap();
        connect_port!(fabric, tx, i => sink, rx).unwrap();
        sinks.push(sink);
    }

    run_simulation!(engine);
    sinks
}

fn default_config() -> Rc<FabricConfig> {
    let num_columns = 3;
    let num_rows = 4;
    let num_ports_per_node = 2;
    let cycles_per_hop = 5;
    let cycles_overhead = 1;
    let rx_buffer_entries = 1;
    let tx_buffer_entries = 1;
    let port_bits_per_tick = 128;

    let config = FabricConfig::new(
        num_columns,
        num_rows,
        num_ports_per_node,
        cycles_per_hop,
        cycles_overhead,
        rx_buffer_entries,
        tx_buffer_entries,
        port_bits_per_tick,
    );
    Rc::new(config)
}

#[test]
fn all_to_one() {
    let num_frames = 100;
    let payload_bytes = 256;

    let config = default_config();
    let num_ports = config.num_ports();

    let to_dest = FixedDest(0);
    let sinks = run_test(config.clone(), &to_dest, num_frames, payload_bytes);

    assert_eq!(sinks[0].num_sunk(), num_ports * num_frames);
    for i in 1..num_ports {
        assert_eq!(sinks[i].num_sunk(), 0);
    }
}

#[test]
fn all_to_all() {
    let num_frames = 100;
    let payload_bytes = 256;

    let config = default_config();
    let num_ports = config.num_ports();

    let to_dest = ToNext { num_ports };
    let sinks = run_test(config.clone(), &to_dest, num_frames, payload_bytes);

    for i in 0..num_ports {
        assert_eq!(sinks[i].num_sunk(), num_frames);
    }
}

#[test]
fn latency() {
    // Test sending a single frame across the fabric
    let payload_bytes = 256;

    let config = default_config();
    let num_ports = config.num_ports();

    let mut engine = start_test(file!());

    let clock = engine.clock_ghz(1.0);
    let spawner = engine.spawner();
    let top = engine.top();

    let fabric = Fabric::new_and_register(
        &engine,
        top,
        "fabric",
        clock.clone(),
        spawner,
        config.clone(),
    )
    .unwrap();

    let mut sources = Vec::with_capacity(num_ports);
    let mut sinks = Vec::with_capacity(num_ports);

    // Connect up sources that will do nothing to all ports
    for i in 0..num_ports {
        let source =
            Source::new_and_register(&engine, top, format!("source{i}").as_str(), None).unwrap();
        connect_port!(source, tx => fabric, rx, i).unwrap();
        sources.push(source);

        let sink = Sink::new_and_register(&engine, top, format!("sink{i}").as_str()).unwrap();
        connect_port!(fabric, tx, i => sink, rx).unwrap();
        sinks.push(sink);
    }

    let num_columns = config.num_columns();
    let num_rows = config.num_rows();

    // Send a single frame from one corner to the other
    let source_index = config.port_index(0, 0, 0);
    let dest_index = config.port_index(
        num_columns - 1,
        num_rows - 1,
        config.num_ports_per_node() - 1,
    );

    let frame = EthernetFrame::new(engine.top(), payload_bytes)
        .set_dest(u64_to_mac(dest_index as u64))
        .set_src(u64_to_mac(source_index as u64));
    let frame_bits = frame.total_bytes() * 8;
    sources[source_index].set_generator(Some(Box::new(vec![frame].into_iter())));

    run_simulation!(engine);

    for i in 0..num_ports {
        if i == dest_index {
            assert_eq!(sinks[i].num_sunk(), 1);
        } else {
            assert_eq!(sinks[i].num_sunk(), 0);
        }
    }

    let ticks_through_limiter = frame_bits.div_ceil(config.port_bits_per_tick());
    let num_hops = (num_columns - 1) + (num_rows - 1);
    let ticks_through_fabric = num_hops * config.cycles_per_hop() + config.cycles_overhead();
    let ticks = ticks_through_limiter + ticks_through_fabric;
    assert_eq!(clock.tick_now().tick(), ticks as u64);
}
