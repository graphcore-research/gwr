// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::flow_controls::rate_limiter::RateLimiter;
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::{SimObject, TotalBytes};
use gwr_track::id::{Id, Unique};

#[derive(Clone, Debug)]
struct Packet(usize);
impl TotalBytes for Packet {
    fn total_bytes(&self) -> usize {
        self.0
    }
}
impl fmt::Display for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "packet({})", self.0)
    }
}
impl Unique for Packet {
    fn id(&self) -> Id {
        Id(0)
    }
}
impl SimObject for Packet {}

#[test]
fn rate_rounds_up_and_accepts_zero_payload() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    for (bits_per_tick, packet_bytes, expected_ticks) in [
        (1, 0, 0),
        (1, 4, 32),
        (7, 1, 2),
        (16, 4, 2),
        (32, 17, 5),
        (65, 129, 16),
    ] {
        let limiter = RateLimiter::new(&clock, bits_per_tick).unwrap();
        assert_eq!(
            limiter.ticks(&Packet(packet_bytes)).unwrap(),
            expected_ticks
        );
        assert_eq!(
            limiter.ticks_from_bits(packet_bytes * 8).unwrap(),
            expected_ticks
        );
    }
    assert!(
        RateLimiter::new(&clock, 1)
            .unwrap()
            .ticks(&Packet(usize::MAX))
            .unwrap_err()
            .to_string()
            .contains("packet bit count overflows")
    );
}

#[test]
fn constructor_rejects_zero_rate() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    match RateLimiter::<Packet>::new(&clock, 0) {
        Err(error) => assert!(error.to_string().contains("rate must be positive")),
        Ok(_) => panic!("zero rate must be rejected during construction"),
    }
}

// Real retained producer put futures: first delivery is earlier than sender
// ACK.
fn arrival_and_ack(receiver_delay: u64) -> (u64, u64) {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let limiter = Limiter::<Packet>::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "limit",
        Rc::new(RateLimiter::new(&clock, 1).unwrap()),
    );
    let mut source = OutPort::new(engine.top(), "source");
    source.connect(limiter.port_rx()).unwrap();
    let mut sink = InPort::new(&engine, &clock, engine.top(), "sink");
    limiter.connect_port_tx(sink.state()).unwrap();
    let arrival = Rc::new(Cell::new(None));
    let ack = Rc::new(Cell::new(None));
    let receiver_arrival = arrival.clone();
    let receiver_clock = clock.clone();
    engine.spawn(async move {
        receiver_clock.wait_ticks(receiver_delay)?.await;
        let value = sink.get()?.await;
        assert_eq!(value.0, 4);
        receiver_arrival.set(Some(receiver_clock.tick_now().tick()));
        Ok(())
    });
    let sender_ack = ack.clone();
    let sender_clock = clock.clone();
    engine.spawn(async move {
        source.put(Packet(4))?.await;
        sender_ack.set(Some(sender_clock.tick_now().tick()));
        Ok(())
    });
    let pending_ack = ack.clone();
    let check_clock = clock.clone();
    engine.spawn(async move {
        check_clock.wait_ticks(receiver_delay + 1)?.await;
        assert_eq!(
            pending_ack.get(),
            None,
            "the actual put future must still await service ACK"
        );
        Ok(())
    });
    engine.run().unwrap();
    (arrival.get().unwrap(), ack.get().unwrap())
}
#[test]
fn first_forwarded_arrival_precedes_rate_ack() {
    assert_eq!(arrival_and_ack(0), (0, 32));
}
#[test]
fn receiver_stall_precedes_service_wait() {
    assert_eq!(arrival_and_ack(5), (5, 37));
}

#[test]
fn packet_bit_overflow_precedes_delivery_and_ack() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let limiter = Limiter::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "limit",
        Rc::new(RateLimiter::new(&clock, 1).unwrap()),
    );
    let mut source = OutPort::new(engine.top(), "source");
    source.connect(limiter.port_rx()).unwrap();
    let mut sink = InPort::new(&engine, &clock, engine.top(), "sink");
    limiter.connect_port_tx(sink.state()).unwrap();
    let delivered = Rc::new(Cell::new(false));
    let acked = Rc::new(Cell::new(false));
    let receiver_delivery = delivered.clone();
    engine.spawn(async move {
        sink.get()?.await;
        receiver_delivery.set(true);
        Ok(())
    });
    let sender_ack = acked.clone();
    engine.spawn(async move {
        source.put(Packet(usize::MAX))?.await;
        sender_ack.set(true);
        Ok(())
    });
    let error = engine.run().unwrap_err();
    assert!(error.to_string().contains("packet bit count overflows"));
    assert!(!delivered.get());
    assert!(!acked.get());
}

// Private arithmetic boundary, not a physically reachable fabric workload
// claim.
#[test]
fn late_deadline_error_keeps_already_delivered_packet_unacked() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let limiter = Limiter::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "limit",
        Rc::new(RateLimiter::new(&clock, 1).unwrap()),
    );
    let mut source = OutPort::new(engine.top(), "source");
    source.connect(limiter.port_rx()).unwrap();
    let mut sink = InPort::new(&engine, &clock, engine.top(), "sink");
    limiter.connect_port_tx(sink.state()).unwrap();
    let delivered = Rc::new(Cell::new(None));
    let acked = Rc::new(Cell::new(false));
    let receiver_delivery = delivered.clone();
    let receiver_clock = clock.clone();
    engine.spawn(async move {
        sink.get()?.await;
        receiver_delivery.set(Some(receiver_clock.tick_now().tick()));
        Ok(())
    });
    let sender_ack = acked.clone();
    let sender_clock = clock.clone();
    engine.spawn(async move {
        sender_clock.wait_ticks(u64::MAX - 16)?.await;
        source.put(Packet(4))?.await;
        sender_ack.set(true);
        Ok(())
    });
    let error = engine.run().unwrap_err();
    assert!(error.to_string().contains("deadline overflows"));
    assert_eq!(delivered.get(), Some(u64::MAX - 16));
    assert!(!acked.get());
}
