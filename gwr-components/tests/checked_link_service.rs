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
    for (rate, bytes, expected) in [
        (1, 0, 0),
        (1, 4, 32),
        (7, 1, 2),
        (16, 4, 2),
        (32, 17, 5),
        (65, 129, 16),
    ] {
        let limiter = RateLimiter::new(&clock, rate).unwrap();
        assert_eq!(limiter.ticks(&Packet(bytes)).unwrap(), expected);
    }
    assert!(
        RateLimiter::<Packet>::new(&clock, 0)
            .err()
            .expect("zero rates must be rejected at construction")
            .to_string()
            .contains("rate must be positive")
    );
    assert!(
        RateLimiter::new(&clock, 1)
            .unwrap()
            .ticks(&Packet(usize::MAX))
            .unwrap_err()
            .to_string()
            .contains("packet bit count overflows")
    );
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
    let a = arrival.clone();
    let c = clock.clone();
    engine.spawn(async move {
        c.wait_ticks(receiver_delay)?.await;
        let value = sink.get()?.await;
        assert_eq!(value.0, 4);
        a.set(Some(c.tick_now().tick()));
        Ok(())
    });
    let a = ack.clone();
    let c = clock.clone();
    engine.spawn(async move {
        source.put(Packet(4))?.await;
        a.set(Some(c.tick_now().tick()));
        Ok(())
    });
    let pending = ack.clone();
    let c = clock.clone();
    engine.spawn(async move {
        c.wait_ticks(receiver_delay + 1)?.await;
        if receiver_delay + 1 < 32 {
            assert_eq!(
                pending.get(),
                None,
                "the actual put future must still await service ACK"
            );
        }
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
fn receiver_stall_overlaps_service_wait() {
    assert_eq!(arrival_and_ack(5), (5, 32));
    assert_eq!(arrival_and_ack(40), (40, 40));
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
    let d = delivered.clone();
    let a = acked.clone();
    engine.spawn(async move {
        sink.get()?.await;
        d.set(true);
        Ok(())
    });
    engine.spawn(async move {
        source.put(Packet(usize::MAX))?.await;
        a.set(true);
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
fn late_deadline_error_precedes_delivery_and_ack() {
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
    let d = delivered.clone();
    let a = acked.clone();
    let c = clock.clone();
    engine.spawn(async move {
        sink.get()?.await;
        d.set(Some(c.tick_now().tick()));
        Ok(())
    });
    let c = clock.clone();
    engine.spawn(async move {
        c.wait_ticks(u64::MAX - 16)?.await;
        source.put(Packet(4))?.await;
        a.set(true);
        Ok(())
    });
    let error = engine.run().unwrap_err();
    assert!(error.to_string().contains("deadline overflows"));
    assert_eq!(delivered.get(), None);
    assert!(!acked.get());
}
