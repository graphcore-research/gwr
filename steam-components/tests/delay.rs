// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use steam_components::delay::Delay;
use steam_components::sink::Sink;
use steam_components::source::Source;
use steam_components::store::Store;
use steam_components::{connect_port, option_box_repeat};
use steam_engine::engine::Engine;
use steam_engine::port::{InPort, OutPort};
use steam_engine::test_helpers::start_test;
use steam_engine::time::clock::Clock;
use steam_engine::traits::SimObject;
use steam_engine::{run_simulation, spawn_simulation};

#[test]
fn put_get() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let spawner = engine.spawner();

    // Create a pair of tasks that use a delay
    let delay = Delay::new(engine.top(), "delay", clock.clone(), spawner.clone(), 20);
    let buffer = Store::new(engine.top(), "buffer", spawner, 1);
    const NUM_PUTS: i32 = 100;

    connect_port!(delay, tx => buffer, rx);
    spawn_simulation!(engine; [delay, buffer]);

    let mut tx = OutPort::new(engine.top(), "tb_tx");
    tx.connect(delay.port_rx());
    engine.spawn(async move {
        for _ in 0..NUM_PUTS {
            let value = 1;
            println!("Push {value}");
            tx.put(value).await?;
        }
        Ok(())
    });

    let rx = InPort::new(engine.top(), "test_rx");
    buffer.connect_port_tx(rx.state());
    let rx_count = Rc::new(RefCell::new(0));
    {
        let rx_count = rx_count.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            for _ in 0..NUM_PUTS {
                let j = rx.get().await;
                let now = clock.tick_now();
                println!("Received {j} @{now}");
                *rx_count.borrow_mut() += j;
            }
            Ok(())
        });
    }

    engine.run().unwrap();

    let now = clock.tick_now();
    let total = *rx_count.borrow();
    println!("Total: {total} @{now}!");
    assert_eq!(total, NUM_PUTS);
}

#[test]
fn source_sink() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let spawner = engine.spawner();

    const DELAY: usize = 3;
    const NUM_PUTS: usize = DELAY * 10;

    let source = Source::new(engine.top(), "source", option_box_repeat!(500 ; NUM_PUTS));
    let delay = Delay::new(engine.top(), "delay", clock, spawner, DELAY);
    let sink = Sink::new(engine.top(), "sink");

    connect_port!(source, tx => delay, rx);
    connect_port!(delay, tx => sink, rx);

    run_simulation!(engine; [source, delay, sink]);

    let num_sunk = sink.num_sunk();
    assert_eq!(num_sunk, NUM_PUTS);
}

#[test]
#[should_panic(expected = "Delay output stalled")]
fn blocked_output() {
    // Cause the delay to raise an assertion because it will find the buffer full
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let spawner = engine.spawner();

    const DELAY: usize = 1;
    const NUM_PUTS: usize = 10;

    let source = Source::new(engine.top(), "source", option_box_repeat!(500 ; NUM_PUTS));
    let delay = Delay::new(engine.top(), "delay", clock.clone(), spawner.clone(), DELAY);
    let store = Store::new(engine.top(), "store", spawner, 1);

    connect_port!(source, tx => delay, rx);
    connect_port!(delay, tx => store, rx);

    spawn_simulation!(engine; [source, store, delay]);
    spawn_slow_reader(&engine, clock, store);
    run_simulation!(engine);
}

fn spawn_slow_reader<T>(engine: &Engine, clock: Clock, store: Store<T>)
where
    T: SimObject,
{
    let rx = InPort::new(engine.top(), "rx");
    store.connect_port_tx(rx.state());
    engine.spawn(async move {
        loop {
            let value = rx.get().await;
            let now = clock.tick_now();
            println!("Pipeline received {} @{now}", value);
            clock.wait_ticks(10).await;
        }
    });
}

#[test]
#[should_panic(expected = "top::delay::tx not connected")]
fn disconnected_delay() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let spawner = engine.spawner();

    const DELAY: usize = 1;
    const NUM_PUTS: usize = 10;

    let source = Source::new(engine.top(), "source", option_box_repeat!(500 ; NUM_PUTS));
    let delay = Delay::new(engine.top(), "delay", clock, spawner, DELAY);

    connect_port!(source, tx => delay, rx);

    run_simulation!(engine; [source, delay]);
}
