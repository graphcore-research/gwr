// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Uncompiled Task2 RED draft; approved caller lane seam, no physical bank
//! claim.
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::task::Context;

use futures::future::poll_fn;
use futures::task::noop_waker;
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::{AccessType, DeviceId, SimError};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::traits::{AccessMemory, ReadMemory};
use gwr_models::memory::{Memory, MemoryConfig, MemoryServiceSnapshot};

// Token is owned by the test, never borrowed from optional tracer IDs.
#[derive(Clone, Debug)]
struct Packet {
    inner: MemoryAccess,
    token: Rc<()>,
}
impl std::fmt::Display for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}
impl gwr_track::id::Unique for Packet {
    fn id(&self) -> gwr_track::Id {
        self.inner.id()
    }
}
impl TotalBytes for Packet {
    fn total_bytes(&self) -> usize {
        self.inner.total_bytes()
    }
}
impl SimObject for Packet {}
impl Routable for Packet {
    fn dst_addr(&self) -> u64 {
        self.inner.dst_addr()
    }
    fn src_addr(&self) -> u64 {
        self.inner.src_addr()
    }
    fn dst_device(&self) -> DeviceId {
        self.inner.dst_device()
    }
    fn src_device(&self) -> DeviceId {
        self.inner.src_device()
    }
    fn access_type(&self) -> AccessType {
        self.inner.access_type()
    }
}
impl AccessMemory for Packet {
    fn access_size_bytes(&self) -> usize {
        self.inner.access_size_bytes()
    }
    fn to_response(&self, memory: &impl ReadMemory) -> Result<Self, SimError> {
        Ok(Self {
            inner: self.inner.to_response(memory)?,
            token: self.token.clone(),
        })
    }
    fn to_ack(&self) -> Result<Self, SimError> {
        Ok(Self {
            inner: self.inner.to_ack()?,
            token: self.token.clone(),
        })
    }
}
#[derive(Debug)]
struct Run {
    accepted: Vec<u64>,
    replies: Vec<(usize, AccessType, u64)>,
    state: MemoryServiceSnapshot,
    stalled: Option<MemoryServiceSnapshot>,
    lane_bytes: Option<Vec<(usize, usize)>>,
    read: usize,
    written: usize,
}
// (actual source release, relative address, payload bytes, access kind).
#[expect(
    clippy::too_many_lines,
    reason = "Keep complete setup execution and assertions of this scenario together"
)]
#[expect(
    clippy::many_single_char_names,
    reason = "Use conventional matrix dimensions and operand names"
)]
fn run(
    inputs: &[(u64, u64, usize, AccessType)],
    lanes: Option<Vec<u64>>,
    budget: usize,
    delay: usize,
    consumer_at: u64,
) -> Run {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let mut config = MemoryConfig::new(0x80000, 65536, 8, delay).with_arrival_aware_service(budget);
    if let Some(rates) = lanes {
        config = config.with_service_lanes(4, rates);
    }
    let memory = Memory::new_and_register(&engine, &clock, engine.top(), "memory", config).unwrap();
    let mut tx = OutPort::new(engine.top(), "requests");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx: InPort<Packet> = InPort::new(&engine, &clock, engine.top(), "responses");
    memory.connect_port_tx(rx.state()).unwrap();
    let tokens = inputs.iter().map(|_| Rc::new(())).collect::<Vec<_>>();
    let packets = inputs
        .iter()
        .zip(&tokens)
        .map(|(&(at, offset, bytes, kind), token)| {
            (
                at,
                Packet {
                    inner: MemoryAccess::new(
                        engine.top(),
                        kind,
                        bytes,
                        0x80000 + offset,
                        0x100,
                        DeviceId(9),
                        DeviceId(7),
                        16,
                    ),
                    token: token.clone(),
                },
            )
        })
        .collect::<Vec<_>>();
    let expected = inputs
        .iter()
        .zip(&tokens)
        .filter(|(i, _)| i.3 != AccessType::WriteRequest)
        .map(|(i, t)| (i.2, i.3, t.clone()))
        .collect::<Vec<_>>();
    let accepted = Rc::new(RefCell::new(vec![]));
    let a = accepted.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        for (at, p) in packets {
            c.wait_ticks(at.saturating_sub(c.tick_now().tick()))?.await;
            tx.put(p)?.await;
            a.borrow_mut().push(c.tick_now().tick());
        }
        Ok(())
    });
    let replies = Rc::new(RefCell::new(vec![]));
    let r = replies.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        c.wait_ticks(consumer_at)?.await;
        for (bytes, kind, token) in expected {
            let p = rx.get()?.await;
            assert!(Rc::ptr_eq(&p.token, &token));
            assert_eq!((p.src_device(), p.dst_device()), (DeviceId(9), DeviceId(7)));
            assert_eq!(p.src_addr(), 0x100);
            assert_eq!(p.access_size_bytes(), bytes);
            let response = if kind == AccessType::ReadRequest {
                AccessType::ReadResponse
            } else {
                AccessType::WriteNonPostedResponse
            };
            assert_eq!(p.access_type(), response);
            assert_eq!(
                p.total_bytes(),
                if kind == AccessType::ReadRequest {
                    bytes + 16
                } else {
                    16
                }
            );
            r.borrow_mut().push((bytes, response, c.tick_now().tick()));
        }
        Ok(())
    });
    let stalled = Rc::new(RefCell::new(None));
    if consumer_at > 6 {
        let s = stalled.clone();
        let m = memory.clone();
        let c = clock.clone();
        engine.spawner().spawn(async move {
            c.wait_ticks(6)?.await;
            *s.borrow_mut() = m.service_snapshot();
            Ok(())
        });
    }
    engine.run().unwrap();
    let result = Run {
        accepted: accepted.borrow().clone(),
        replies: replies.borrow().clone(),
        state: memory.service_snapshot().unwrap(),
        stalled: *stalled.borrow(),
        lane_bytes: memory.service_lane_snapshot().map(|v| {
            v.iter()
                .map(|s| (s.payload_bytes_read, s.payload_bytes_written))
                .collect()
        }),
        read: memory.bytes_read(),
        written: memory.bytes_written(),
    };
    assert_eq!(result.state.total, 0);
    assert_eq!(
        result.state.accepted_requests,
        u64::try_from(inputs.len()).unwrap()
    );
    assert_eq!(
        result.state.retired_requests,
        u64::try_from(inputs.len()).unwrap()
    );
    result
}
#[test]
fn actual_same_lane_and_balanced_arrivals_have_distinct_service_not_payload() {
    let read = AccessType::ReadRequest;
    let same = run(
        &[(0, 0, 4, read), (0, 8, 4, read)],
        Some(vec![4, 4]),
        2,
        0,
        0,
    );
    let balanced = run(
        &[(0, 0, 4, read), (0, 4, 4, read)],
        Some(vec![4, 4]),
        2,
        0,
        0,
    );
    assert_eq!(same.accepted, vec![0, 0]);
    assert_eq!(balanced.accepted, vec![0, 0]);
    assert_eq!(
        same.replies.iter().map(|r| r.2).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        balanced.replies.iter().map(|r| r.2).collect::<Vec<_>>(),
        vec![1, 1]
    );
    assert_eq!((same.read, balanced.read), (8, 8));
    assert_eq!(same.lane_bytes, Some(vec![(8, 0), (0, 0)]));
    assert_eq!(balanced.lane_bytes, Some(vec![(4, 0), (4, 0)]));
}
#[test]
fn multi_stripe_parent_has_one_response_and_waits_for_all_lanes() {
    let r = run(
        &[(0, 3, 6, AccessType::ReadRequest)],
        Some(vec![4, 4]),
        1,
        3,
        0,
    );
    assert_eq!(r.replies, vec![(6, AccessType::ReadResponse, 4)]);
    assert_eq!(r.lane_bytes, Some(vec![(2, 0), (4, 0)]));
    assert_eq!(r.state.peak_total, 1);
}
#[test]
fn output_ack_stall_retains_parent_budget_at_zero_and_positive_delay() {
    for delay in [0, 3] {
        let r = run(
            &[(0, 0, 4, AccessType::ReadRequest); 4],
            Some(vec![4, 4]),
            2,
            delay,
            10,
        );
        assert_eq!(r.accepted, vec![0, 0, 10, 10]);
        assert_eq!(
            r.replies.iter().map(|r| r.2).collect::<Vec<_>>(),
            vec![10, 10, 11 + delay as u64, 12 + delay as u64]
        );
        let s = r.stalled.unwrap();
        assert_eq!((s.total, s.completed, s.peak_total), (2, 2, 2));
        assert_eq!(r.state.peak_total, 2);
    }
}
#[test]
fn posted_write_budget_and_late_arrival_never_use_response_or_idle_credit() {
    let posted = run(
        &[(0, 0, 4, AccessType::WriteRequest); 4],
        Some(vec![4, 4]),
        1,
        200,
        0,
    );
    assert_eq!(posted.accepted, vec![0, 1, 2, 3]);
    assert!(posted.replies.is_empty());
    assert_eq!((posted.read, posted.written), (0, 16));
    assert_eq!(posted.state.peak_total, 1);
    let late = run(
        &[
            (0, 0, 4, AccessType::ReadRequest),
            (10, 4, 4, AccessType::WriteNonPostedRequest),
        ],
        Some(vec![4, 4]),
        2,
        3,
        0,
    );
    assert_eq!(late.accepted, vec![0, 10]);
    assert_eq!(
        late.replies.iter().map(|r| r.2).collect::<Vec<_>>(),
        vec![4, 14]
    );
    assert_eq!(late.lane_bytes, Some(vec![(4, 0), (0, 4)]));
}
#[test]
fn one_lane_identity_preserves_existing_arrival_service_observations() {
    let inputs = [
        (0, 0, 3, AccessType::ReadRequest),
        (0, 4, 7, AccessType::WriteNonPostedRequest),
        (10, 8, 8, AccessType::ReadRequest),
    ];
    let old = run(&inputs, None, 3, 3, 0);
    let one = run(&inputs, Some(vec![8]), 3, 3, 0);
    assert_eq!(old.accepted, one.accepted);
    assert_eq!(old.replies, one.replies);
    assert_eq!(old.state, one.state);
    assert_eq!((old.read, old.written), (one.read, one.written));
    assert!(old.lane_bytes.is_none());
    assert_eq!(one.lane_bytes, Some(vec![(11, 7)]));
}
#[test]
fn invalid_real_requests_never_acknowledge_or_commit_parent_or_lane_counters() {
    for (kind, bytes, address) in [
        (AccessType::ReadRequest, 0, 0x80000),
        (AccessType::WriteRequest, 0, 0x80000),
        (AccessType::ReadRequest, 1, 0x7ffff),
        (AccessType::ReadRequest, 2, 0x8ffff),
        (AccessType::Control, 1, 0x80000),
        (AccessType::ReadRequest, 2, u64::MAX),
    ] {
        let mut e = Engine::new(&gwr_track::tracker::dev_null_tracker());
        let c = e.default_clock();
        let m = Memory::new_and_register(
            &e,
            &c,
            e.top(),
            "memory",
            MemoryConfig::new(0x80000, 65536, 8, 0)
                .with_arrival_aware_service(1)
                .with_service_lanes(4, vec![4, 4]),
        )
        .unwrap();
        let p = Packet {
            inner: MemoryAccess::new(
                e.top(),
                kind,
                bytes,
                address,
                0x100,
                DeviceId(9),
                DeviceId(7),
                16,
            ),
            token: Rc::new(()),
        };
        let mut tx = OutPort::new(e.top(), "requests");
        tx.connect(m.port_rx()).unwrap();
        let rx: InPort<Packet> = InPort::new(&e, &c, e.top(), "responses");
        m.connect_port_tx(rx.state()).unwrap();
        let acknowledged = Rc::new(Cell::new(false));
        let ack = acknowledged.clone();
        let retained_put = Rc::new(RefCell::new(Box::pin(tx.put(p).unwrap())));
        let sender_put = retained_put.clone();
        e.spawner().spawn(async move {
            tx.try_put()?.await;
            poll_fn(|cx| sender_put.borrow_mut().as_mut().poll(cx)).await;
            ack.set(true);
            Ok(())
        });
        assert!(e.run().is_err());
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        assert!(
            retained_put
                .borrow_mut()
                .as_mut()
                .poll(&mut context)
                .is_pending(),
            "the actual producer future must remain unacknowledged after intake error"
        );
        assert!(!acknowledged.get());
        assert_eq!(
            m.service_snapshot().unwrap(),
            MemoryServiceSnapshot::default()
        );
        assert_eq!((m.bytes_read(), m.bytes_written()), (0, 0));
        assert_eq!(
            m.service_lane_snapshot()
                .unwrap()
                .iter()
                .map(|s| (s.payload_bytes_read, s.payload_bytes_written))
                .collect::<Vec<_>>(),
            vec![(0, 0), (0, 0)]
        );
    }
}

#[test]
fn later_fast_lane_parent_keeps_fifo_publication_order() {
    let r = run(
        &[
            (0, 0, 4, AccessType::ReadRequest),
            (0, 4, 4, AccessType::ReadRequest),
        ],
        Some(vec![1, 7]),
        2,
        0,
        0,
    );
    assert_eq!(r.accepted, vec![0, 0]);
    assert_eq!(
        r.replies.iter().map(|r| r.2).collect::<Vec<_>>(),
        vec![4, 4]
    );
    assert_eq!(r.lane_bytes, Some(vec![(4, 0), (4, 0)]));
}

#[test]
fn omitted_legacy_and_unlaned_service_preserve_real_zero_request_rejection() {
    for arrival in [false, true] {
        for kind in [AccessType::ReadRequest, AccessType::WriteRequest] {
            let mut e = Engine::new(&gwr_track::tracker::dev_null_tracker());
            let c = e.default_clock();
            let mut config = MemoryConfig::new(0x80000, 65536, 8, 0);
            if arrival {
                config = config.with_arrival_aware_service(1);
            }
            let m = Memory::new_and_register(&e, &c, e.top(), "memory", config).unwrap();
            let p = Packet {
                inner: MemoryAccess::new(
                    e.top(),
                    kind,
                    0,
                    0x80000,
                    0x100,
                    DeviceId(9),
                    DeviceId(7),
                    16,
                ),
                token: Rc::new(()),
            };
            let mut tx = OutPort::new(e.top(), "requests");
            tx.connect(m.port_rx()).unwrap();
            let rx: InPort<Packet> = InPort::new(&e, &c, e.top(), "responses");
            m.connect_port_tx(rx.state()).unwrap();
            let acknowledged = Rc::new(Cell::new(false));
            let ack = acknowledged.clone();
            let retained_put = Rc::new(RefCell::new(Box::pin(tx.put(p).unwrap())));
            let sender_put = retained_put.clone();
            e.spawner().spawn(async move {
                tx.try_put()?.await;
                poll_fn(|cx| sender_put.borrow_mut().as_mut().poll(cx)).await;
                ack.set(true);
                Ok(())
            });
            assert!(e.run().is_err());
            // Legacy get() releases producer before validation; preserve that
            // path, do not mistake an unpolled sender for atomic pre-ACK
            // intake.
            if arrival {
                let waker = noop_waker();
                let mut context = Context::from_waker(&waker);
                assert!(
                    retained_put
                        .borrow_mut()
                        .as_mut()
                        .poll(&mut context)
                        .is_pending()
                );
                assert!(!acknowledged.get());
            }
            assert_eq!((m.bytes_read(), m.bytes_written()), (0, 0));
            if arrival {
                assert_eq!(
                    m.service_snapshot().unwrap(),
                    MemoryServiceSnapshot::default()
                );
            } else {
                assert!(m.service_snapshot().is_none());
            }
            assert!(m.service_lane_snapshot().is_none());
        }
    }
}
