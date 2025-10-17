// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use gwr_engine::port::{InPort, OutPort};
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;

#[test]
#[should_panic(expected = "top::tx not connected")]
fn disconnected_outport() {
    let mut engine = start_test(file!());

    let tx_port = OutPort::new(engine.top(), "tx");
    engine.spawn(async move {
        tx_port.put(1)?.await;
        Ok(())
    });
    run_simulation!(engine);
}

#[test]
#[should_panic(expected = "top::tx not connected")]
fn disconnected_outport_try_put() {
    let mut engine = start_test(file!());

    let tx_port = OutPort::new(engine.top(), "tx");
    engine.spawn(async move {
        tx_port.try_put()?.await;
        tx_port.put(1)?.await;
        Ok(())
    });
    run_simulation!(engine);
}

#[test]
#[should_panic(expected = "top::rx not connected")]
fn disconnected_input() {
    let mut engine = start_test(file!());

    let rx_port = InPort::new(engine.top(), "rx");
    engine.spawn(async move {
        let _: i32 = rx_port.get()?.await;
        Ok(())
    });
    run_simulation!(engine);
}

#[test]
#[should_panic(expected = "top::rx not connected")]
fn disconnected_input_start() {
    let mut engine = start_test(file!());

    let rx_port = InPort::new(engine.top(), "rx");
    engine.spawn(async move {
        let _: i32 = rx_port.start_get()?.await;
        rx_port.finish_get();
        Ok(())
    });
    run_simulation!(engine);
}
