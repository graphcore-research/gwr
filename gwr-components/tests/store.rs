// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::store::Store;
use gwr_components::{connect_port, option_box_repeat};
use gwr_engine::port::InPort;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_track::entity::GetEntity;

/// Basic end-to-end test: Source → Store → Sink.
///
/// Verifies:
///  * all values make it through the store
///  * the store is empty at the end (fill_level == 0)
#[test]
fn store_basic_flow() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    const NUM_PUTS: usize = 50;
    const CAPACITY: usize = 8;

    let top = engine.top();

    // Simple source that repeatedly produces the same value.
    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(1 ; NUM_PUTS)).unwrap();

    let store = Store::new_and_register(&engine, &clock, top, "store", CAPACITY).unwrap();

    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    // Wire up the simple pipeline: source → store → sink
    connect_port!(source, tx => store, rx).unwrap();
    connect_port!(store, tx => sink, rx).unwrap();

    run_simulation!(engine);

    // All items should have been sunk.
    assert_eq!(sink.num_sunk(), NUM_PUTS);
    // Store must be empty at the end of simulation.
    assert_eq!(store.fill_level(), 0);
}

/// Creating a store with zero capacity should fail with a SimError.
///
/// This directly exercises the constructor error path.
#[test]
fn store_zero_capacity_fails() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let top = engine.top();

    let result = Store::<i32>::new_and_register(&engine, &clock, top, "store_zero", 0);

    assert!(
        result.is_err(),
        "Expected zero-capacity Store construction to return an error"
    );

    let err = result.err().unwrap();
    let msg = err.to_string(); // Display impl prefixes with "Error: "
    assert!(
        msg.contains("Unsupported Store with 0 capacity"),
        "Unexpected error message: {msg}"
    );
}

/// When `set_error_on_overflow` is enabled, overflowing the store should
/// cause the simulation to fail with an overflow error.
///
/// We connect a source that keeps pushing data into the store and
/// connect the store's `tx` port to a port that never takes out data.
/// Expect the overflow path in `State::push_value` to be hit.
///
/// NOTE: we only match a substring of the message to avoid depending
/// on the exact formatting of the entity name.
#[test]
#[should_panic(expected = "Overflow in")]
fn store_overflow_panics_when_error_on_overflow_set() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    const CAPACITY: usize = 2;
    const NUM_PUTS: usize = 10;

    let top = engine.top();

    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(1 ; NUM_PUTS)).unwrap();

    let store = Store::new_and_register(&engine, &clock, top, "store_overflow", CAPACITY).unwrap();

    // Switch to "error on overflow" mode so `run_rx` no longer blocks once full
    // and instead allows `State::push_value` to return a SimError.
    store.set_error_on_overflow();

    // Only connect source → store. No consumer on `store.tx`, so the internal
    // queue keeps growing until it overflows.
    connect_port!(source, tx => store, rx).unwrap();

    // Connect the output of the store to prevent `tx not connected` error.
    let rx = InPort::new(&engine, &clock, engine.top(), "test_rx");
    store.connect_port_tx(rx.state()).unwrap();

    // This should panic due to the SimError bubbling up from the store.
    run_simulation!(engine);
}
