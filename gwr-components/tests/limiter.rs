// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
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

// Retain real producer put futures to observe both arrival and service
// completion.
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
        check_clock.wait_ticks(receiver_delay.max(32) - 1)?.await;
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
fn receiver_stall_overlaps_service_wait() {
    assert_eq!(arrival_and_ack(5), (5, 32));
    assert_eq!(arrival_and_ack(40), (40, 40));
}

#[test]
fn chaining_equal_rate_limiters_preserves_bandwidth() {
    // A four-byte packet at two bits per tick takes 16 ticks, regardless of
    // chain length. Adding service waits after forwarding would multiply this.
    for rates in [&[2][..], &[2, 2], &[2, 2, 2, 2]] {
        let (arrivals, acks) = chain_timings(rates, 4);
        assert_eq!(
            arrivals,
            [0, 16, 32, 48, 64, 80, 96, 112],
            "rates={rates:?}"
        );
        assert_eq!(acks, [16, 32, 48, 64, 80, 96, 112, 128], "rates={rates:?}");
    }
}

#[test]
fn chained_bandwidth_is_the_lowest_rate_in_every_order() {
    // The one-bit-per-tick limiter sets a 32-tick interval in all six orders.
    for rates in [
        [1, 2, 4],
        [1, 4, 2],
        [2, 1, 4],
        [2, 4, 1],
        [4, 1, 2],
        [4, 2, 1],
    ] {
        let (arrivals, acks) = chain_timings(&rates, 4);
        assert_eq!(
            arrivals,
            [0, 32, 64, 96, 128, 160, 192, 224],
            "rates={rates:?}"
        );
        assert_eq!(
            acks,
            [32, 64, 96, 128, 160, 192, 224, 256],
            "rates={rates:?}"
        );
    }
}

#[test]
fn chained_bandwidth_rounds_packet_service_up_to_whole_ticks() {
    // 32 bits / 3 bits per tick rounds up to 11 ticks; the five-bit rate
    // rounds to seven ticks and must not add another delay to the interval.
    for rates in [[3, 5], [5, 3]] {
        let (arrivals, acks) = chain_timings(&rates, 4);
        assert_eq!(arrivals, [0, 11, 22, 33, 44, 55, 66, 77], "rates={rates:?}");
        assert_eq!(acks, [11, 22, 33, 44, 55, 66, 77, 88], "rates={rates:?}");
    }
}

// A continuously ready source and sink isolate the chain's service bandwidth.
// Record both ends: the first arrival is immediate, but each put must wait for
// service to complete before the source can send the next packet.
fn chain_timings(bits_per_tick: &[usize], packet_bytes: usize) -> (Vec<u64>, Vec<u64>) {
    const PACKETS: usize = 8;
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let limiters: Vec<_> = bits_per_tick
        .iter()
        .enumerate()
        .map(|(index, &rate)| {
            Limiter::<Packet>::new_and_register(
                &engine,
                &clock,
                engine.top(),
                &format!("limit_{index}"),
                Rc::new(RateLimiter::new(&clock, rate).unwrap()),
            )
        })
        .collect();
    let mut source = OutPort::new(engine.top(), "source");
    source.connect(limiters.first().unwrap().port_rx()).unwrap();
    for pair in limiters.windows(2) {
        pair[0].connect_port_tx(pair[1].port_rx()).unwrap();
    }
    let mut sink = InPort::new(&engine, &clock, engine.top(), "sink");
    limiters
        .last()
        .unwrap()
        .connect_port_tx(sink.state())
        .unwrap();

    let arrivals = Rc::new(RefCell::new(Vec::new()));
    let receiver_arrivals = arrivals.clone();
    let receiver_clock = clock.clone();
    engine.spawn(async move {
        for _ in 0..PACKETS {
            let packet = sink.get()?.await;
            assert_eq!(packet.0, packet_bytes);
            receiver_arrivals
                .borrow_mut()
                .push(receiver_clock.tick_now().tick());
        }
        Ok(())
    });
    let acks = Rc::new(RefCell::new(Vec::new()));
    let sender_acks = acks.clone();
    engine.spawn(async move {
        for _ in 0..PACKETS {
            source.put(Packet(packet_bytes))?.await;
            sender_acks.borrow_mut().push(clock.tick_now().tick());
        }
        Ok(())
    });
    engine.run().unwrap();
    (arrivals.take(), acks.take())
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
fn service_deadline_overflow_precedes_delivery_and_ack() {
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
    assert_eq!(delivered.get(), None);
    assert!(!acked.get());
}
