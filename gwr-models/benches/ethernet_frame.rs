// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use criterion::{BatchSize, Criterion, criterion_group};
use gwr_components::connect_port;
use gwr_components::sink::Sink;
use gwr_components::store::Store;
use gwr_engine::engine::Engine;
use gwr_engine::port::OutPort;
use gwr_engine::traits::SimObject;
use gwr_models::ethernet_frame::{EthernetFrame, SRC_MAC_BYTES};
use gwr_track::tracker::dev_null_tracker;

const NUM_FRAMES: usize = 500;

fn create_engine() -> Engine {
    // Create an engine without the tracker system opening files for logging
    let tracker = dev_null_tracker();
    Engine::new(&tracker)
}

fn run_engine<T>(args: (Engine, Rc<Sink<T>>))
where
    T: SimObject,
{
    let (mut engine, sink) = args;
    engine.run().unwrap();
    assert_eq!(sink.num_sunk(), NUM_FRAMES);
}

fn setup_frame_simulation() -> (Engine, Rc<Sink<EthernetFrame>>) {
    let num_frames = NUM_FRAMES;
    let payload_size_bytes = 256;
    let frame_dest = [0, 1, 2, 3, 4, 5];

    let mut engine = create_engine();
    let clock = engine.default_clock();
    let top = engine.top();
    let mut ring_frames = Vec::with_capacity(num_frames);
    for i in 0..num_frames {
        let frame = EthernetFrame::new(top, payload_size_bytes)
            .set_dest(frame_dest)
            .set_src([i as u8; SRC_MAC_BYTES]);
        ring_frames.push(frame);
    }

    let store_capacity = num_frames / 4;
    let store = Store::new_and_register(&engine, &clock, top, "store", store_capacity).unwrap();

    {
        let mut frame_tx = OutPort::new(engine.top(), "frame_tx");
        frame_tx.connect(store.port_rx()).unwrap();
        engine.spawn(async move {
            for frame in ring_frames.drain(..) {
                frame_tx.put(frame)?.await;
            }
            Ok(())
        });
    }

    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    connect_port!(store, tx => sink, rx).unwrap();

    (engine, sink)
}

fn setup_box_frame_simulation() -> (Engine, Rc<Sink<Box<EthernetFrame>>>) {
    let num_frames = NUM_FRAMES;
    let payload_size_bytes = 256;
    let frame_dest = [0, 1, 2, 3, 4, 5];

    let mut engine = create_engine();
    let clock = engine.default_clock();
    let top = engine.top();
    let mut ring_frames = Vec::with_capacity(num_frames);
    for i in 0..num_frames {
        let frame = EthernetFrame::new(top, payload_size_bytes)
            .set_dest(frame_dest)
            .set_src([i as u8; SRC_MAC_BYTES]);
        ring_frames.push(Box::new(frame));
    }

    let store_capacity = num_frames / 4;
    let store = Store::new_and_register(&engine, &clock, top, "store", store_capacity).unwrap();

    {
        let mut frame_tx = OutPort::new(engine.top(), "frame_tx");
        frame_tx.connect(store.port_rx()).unwrap();
        engine.spawn(async move {
            for frame in ring_frames.drain(..) {
                frame_tx.put(frame)?.await;
            }
            Ok(())
        });
    }

    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    connect_port!(store, tx => sink, rx).unwrap();

    (engine, sink)
}

fn bench_ethernet_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("ethernet_frame");

    group.bench_function("vec_of_frame", |b| {
        b.iter_batched(setup_frame_simulation, run_engine, BatchSize::SmallInput);
    });

    group.bench_function("vec_of_box", |b| {
        b.iter_batched(
            setup_box_frame_simulation,
            run_engine,
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = bench_ethernet_frame
}
