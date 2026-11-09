// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::Routable;
use gwr_engine::types::{AccessType, DeviceId};
use gwr_models::fabric::service::RouteService;
use gwr_models::memory::memory_access::MemoryAccess;

fn run_service(
    count: usize,
    bytes: usize,
    capacity: usize,
    receive_after: u64,
    queue_only: bool,
) -> (
    Vec<u64>,
    Vec<u64>,
    gwr_models::fabric::service::RouteServiceSnapshot,
) {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let service = if queue_only {
        RouteService::<MemoryAccess>::new_queue_and_register(
            &engine,
            &clock,
            engine.top(),
            "queue",
            capacity * bytes.max(1),
            capacity,
        )
        .unwrap()
    } else {
        RouteService::<MemoryAccess>::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "route",
            435,
            3,
            capacity * bytes.max(1),
            capacity,
        )
        .unwrap()
    };
    let mut source = OutPort::new(engine.top(), "source");
    source.connect(service.port_rx()).unwrap();
    let mut sink = InPort::new(&engine, &clock, engine.top(), "sink");
    service.connect_port_tx(sink.state()).unwrap();
    let packets: Vec<_> = (0..count)
        .map(|i| {
            MemoryAccess::new(
                engine.top(),
                AccessType::ReadResponse,
                bytes,
                i as u64,
                0,
                DeviceId(0),
                DeviceId(1),
                0,
            )
        })
        .collect();
    let arrivals = Rc::new(RefCell::new(Vec::new()));
    let a = arrivals.clone();
    let c = clock.clone();
    engine.spawn(async move {
        c.wait_ticks(receive_after)?.await;
        for i in 0..count {
            let p = sink.get()?.await;
            assert_eq!(p.dst_addr(), i as u64);
            a.borrow_mut().push(c.tick_now().tick());
        }
        Ok(())
    });
    let acks = Rc::new(RefCell::new(Vec::new()));
    let a = acks.clone();
    let c = clock.clone();
    engine.spawn(async move {
        for p in packets {
            source.put(p)?.await;
            a.borrow_mut().push(c.tick_now().tick());
        }
        Ok(())
    });
    engine.run().unwrap();

    (
        arrivals.borrow().clone(),
        acks.borrow().clone(),
        service.snapshot(),
    )
}
#[test]
fn fractional_serialization_preserves_debt_across_packets() {
    let (arrivals, _, s) = run(435, 64, 512, 0);
    assert_eq!(*arrivals.last().unwrap(), 515);
    assert_eq!(s.completed_packets, 435);
    assert_eq!(s.admitted_payload_bytes, Some(435 * 64));
    assert_eq!(s.completed_payload_bytes, Some(435 * 64));
    assert_eq!(s.admitted_header_bytes, Some(0));
    assert_eq!(s.completed_header_bytes, Some(0));
    assert_eq!(s.owned_packets, 0);
    assert_eq!(s.owned_bytes, 0);
}
#[test]
fn stalled_receiver_retains_credits_until_ack() {
    let (arrivals, acks, s) = run(8, 64, 2, 100);
    assert!(arrivals.iter().all(|t| *t >= 100));
    assert!(acks[2] >= 100);
    assert_eq!(s.peak_packets, 2);
    assert_eq!(s.owned_packets, 0);
}
#[test]
fn zero_transport_bytes_still_consume_packet_credit() {
    let (_, acks, s) = run(8, 0, 2, 100);
    assert!(acks[2] >= 100);
    assert_eq!(s.peak_packets, 2);
    assert_eq!(s.completed_packets, 8);
}

#[test]
fn oversized_packet_rejects_without_consuming_queue_ownership() {
    let mut engine = Engine::default();
    let c = engine.default_clock();
    let s = RouteService::<MemoryAccess>::new_and_register(
        &engine,
        &c,
        engine.top(),
        "route",
        435,
        0,
        32,
        2,
    )
    .unwrap();
    let mut source = OutPort::new(engine.top(), "source");
    source.connect(s.port_rx()).unwrap();
    let sink = InPort::<MemoryAccess>::new(&engine, &c, engine.top(), "sink");
    s.connect_port_tx(sink.state()).unwrap();
    let packet = MemoryAccess::new(
        engine.top(),
        AccessType::ReadResponse,
        64,
        0,
        0,
        DeviceId(0),
        DeviceId(1),
        0,
    );
    engine.spawn(async move {
        source.put(packet)?.await;
        Ok(())
    });
    assert!(
        engine
            .run()
            .unwrap_err()
            .to_string()
            .contains("exceeds queue capacity")
    );
    assert_eq!(s.snapshot().admitted_packets, 0);
    assert_eq!(s.snapshot().owned_packets, 0);
}

fn run(
    count: usize,
    bytes: usize,
    capacity: usize,
    receive_after: u64,
) -> (
    Vec<u64>,
    Vec<u64>,
    gwr_models::fabric::service::RouteServiceSnapshot,
) {
    run_service(count, bytes, capacity, receive_after, false)
}
#[test]
fn external_queue_has_joint_bounds_without_serialization() {
    let (arrivals, _, s) = run_service(435, 64, 512, 0, true);
    assert_eq!(*arrivals.last().unwrap(), 0);
    assert_eq!(s.service_ticks, 0);
    let (_, acks, s) = run_service(128, 0, 2, 100, true);
    assert!(acks[2] >= 100);
    assert_eq!(s.peak_packets, 2);
    assert_eq!(s.owned_packets, 0);
}
