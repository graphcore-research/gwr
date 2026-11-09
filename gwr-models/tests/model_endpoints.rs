// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::Routable;
use gwr_engine::types::AccessType;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::traits::AccessMemory;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::model::{MemoryEndpoint, MemoryModel};
use gwr_models::test_helpers::{create_default_memory_map, create_read};
use gwr_track::tracker::dev_null_tracker;

#[test]
fn endpoint_inspection_does_not_claim_detailed_ports() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let memory = Memory::<MemoryAccess>::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "ram",
        MemoryConfig::new(0, 4096, 32, 2),
    )
    .unwrap();
    drop(memory.endpoint());
    let detailed = memory.endpoint();
    let rx = InPort::new(&engine, &clock, engine.top(), "client_rx");
    let tx = RefCell::new(OutPort::new(engine.top(), "client_tx"));
    let client = MemoryEndpoint::detailed(|| rx.state(), |p| tx.borrow_mut().connect(p));
    client.connect_detailed(&detailed).unwrap();
    assert!(client.connect_detailed(&detailed).is_err());
    assert!(
        memory.port_rx().is_err(),
        "successful wiring claims the receive port"
    );
}

#[test]
fn detailed_endpoints_carry_a_real_memory_request_and_response() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let memory = Memory::<MemoryAccess>::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "ram",
        MemoryConfig::new(0, 4096, 32, 2),
    )
    .unwrap();
    let mut rx = InPort::new(&engine, &clock, engine.top(), "client_rx");
    let tx = RefCell::new(OutPort::new(engine.top(), "client_tx"));
    {
        let client = MemoryEndpoint::detailed(|| rx.state(), |p| tx.borrow_mut().connect(p));
        client.connect_detailed(&memory.endpoint()).unwrap();
    }
    let map = Rc::new(create_default_memory_map());
    let request = create_read(engine.top(), &map, 64, 0, 1024, 8);
    let mut tx = tx.into_inner();
    engine.spawn(async move {
        tx.put(request)?.await;
        let response = rx.get()?.await;
        assert_eq!(response.access_type(), AccessType::ReadResponse);
        assert_eq!(response.access_size_bytes(), 64);
        // Responses retain addresses; routing reverses the device IDs instead.
        assert_eq!(response.dst_addr(), 0);
        assert_eq!(response.src_addr(), 1024);
        Ok(())
    });
    engine.run().unwrap();
    let model: &dyn MemoryModel = memory.as_ref();
    assert_eq!(model.bytes_read(), 64);
    assert_eq!(model.bytes_written(), 0);
}
