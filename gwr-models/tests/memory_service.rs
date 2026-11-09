// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Optional payload service; observations are analytic simulation boundaries.
use std::cell::RefCell;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::Routable;
use gwr_engine::types::AccessType;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::traits::AccessMemory;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{create_default_memory_map, create_read};

#[test]
fn accepted_burst_shares_ticks_and_preserves_each_response() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let memory = Memory::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "memory",
        MemoryConfig::new(0x80000, 65536, 3350, 3).with_arrival_aware_service(32),
    )
    .unwrap();
    let mut tx = OutPort::new(engine.top(), "requests");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx: InPort<MemoryAccess> = InPort::new(&engine, &clock, engine.top(), "responses");
    memory.connect_port_tx(rx.state()).unwrap();
    let accepted = Rc::new(RefCell::new(Vec::new()));
    let received = Rc::new(RefCell::new(Vec::new()));
    let input_log = accepted.clone();
    let input_clock = clock.clone();
    let map = Rc::new(create_default_memory_map());
    let packets = (0..32)
        .map(|i| create_read(engine.top(), &map, 128, 0x80000 + i * 128, 0x81000, 16))
        .collect::<Vec<_>>();
    engine.spawner().spawn(async move {
        for packet in packets {
            tx.put(packet)?.await;
            input_log.borrow_mut().push(input_clock.tick_now().tick());
        }
        Ok(())
    });
    let output_log = received.clone();
    let output_clock = clock.clone();
    engine.spawner().spawn(async move {
        for _ in 0..32 {
            let response = rx.get()?.await;
            assert_eq!(response.access_type(), AccessType::ReadResponse);
            assert_eq!(response.access_size_bytes(), 128);
            output_log
                .borrow_mut()
                .push((response.dst_addr(), output_clock.tick_now().tick()));
        }
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(
        *accepted.borrow(),
        vec![0; 32],
        "actual memory acceptance, not offered source time"
    );
    let replies = received.borrow();
    assert_eq!(replies.len(), 32);
    for (index, &(address, tick)) in replies.iter().enumerate() {
        assert_eq!(address, 0x80000 + index as u64 * 128);
        assert_eq!(tick, ((index as u64 + 1) * 128).div_ceil(3350) + 3);
    }
    assert_eq!(replies.last().unwrap().1, 5);
    assert_eq!(memory.bytes_read(), 4096);
    assert_eq!(memory.bytes_written(), 0);
    let snapshot = memory.service_snapshot().unwrap();
    assert_eq!(snapshot.total, 0);
    assert_eq!(snapshot.peak_total, 32);
    assert_eq!(snapshot.accepted_requests, 32);
    assert_eq!(snapshot.retired_requests, 32);
}

type Observation = (u64, usize, AccessType, u64);
#[expect(
    clippy::too_many_lines,
    reason = "Keep complete setup execution and assertions of this scenario together"
)]
#[expect(
    clippy::many_single_char_names,
    reason = "Use conventional matrix dimensions and operand names"
)]
fn run_requests(
    inputs: &[(u64, usize, AccessType)],
    budget: Option<usize>,
    delay: usize,
    consumer_start: u64,
) -> (
    Vec<u64>,
    Vec<Observation>,
    Option<gwr_models::memory::MemoryServiceSnapshot>,
    Option<gwr_models::memory::MemoryServiceSnapshot>,
    usize,
    usize,
) {
    use gwr_engine::traits::TotalBytes;
    use gwr_engine::types::DeviceId;
    use gwr_models::memory::memory_map::MemoryMap;
    use gwr_models::test_helpers::{create_write, create_write_np};
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let base = MemoryConfig::new(0x80000, 65536, 3350, delay);
    let cfg = budget.map_or_else(
        || base.clone(),
        |n| base.clone().with_arrival_aware_service(n),
    );
    let memory = Memory::new_and_register(&engine, &clock, engine.top(), "memory", cfg).unwrap();
    let mut tx = OutPort::new(engine.top(), "requests");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx: InPort<MemoryAccess> = InPort::new(&engine, &clock, engine.top(), "responses");
    memory.connect_port_tx(rx.state()).unwrap();
    let mut map = MemoryMap::new();
    map.insert(0, 0x80000, DeviceId(7)).unwrap();
    map.insert(0x80000, 65536, DeviceId(9)).unwrap();
    let map = Rc::new(map);
    let mut offset = 0;
    let packets = inputs
        .iter()
        .map(|&(at, bytes, kind)| {
            let address = 0x80000 + offset;
            offset += bytes as u64;
            let p = match kind {
                AccessType::ReadRequest => {
                    create_read(engine.top(), &map, bytes, address, 0x100, 16)
                }
                AccessType::WriteRequest => {
                    create_write(engine.top(), &map, bytes, address, 0x100, 16)
                }
                AccessType::WriteNonPostedRequest => {
                    create_write_np(engine.top(), &map, bytes, address, 0x100, 16)
                }
                _ => panic!("test input kind"),
            };
            (at, p)
        })
        .collect::<Vec<_>>();
    let responses = inputs
        .iter()
        .filter(|(_, _, k)| *k != AccessType::WriteRequest)
        .count();
    let accepted = Rc::new(RefCell::new(Vec::new()));
    let received = Rc::new(RefCell::new(Vec::new()));
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
    let r = received.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        c.wait_ticks(consumer_start)?.await;
        for _ in 0..responses {
            let p = rx.get()?.await;
            assert_eq!(p.src_device(), DeviceId(9));
            assert_eq!(p.dst_device(), DeviceId(7));
            assert_eq!(p.src_addr(), 0x100);
            assert_eq!(
                p.total_bytes(),
                if p.access_type() == AccessType::ReadResponse {
                    p.access_size_bytes() + 16
                } else {
                    16
                }
            );
            r.borrow_mut().push((
                p.dst_addr(),
                p.access_size_bytes(),
                p.access_type(),
                c.tick_now().tick(),
            ));
        }
        Ok(())
    });
    let probe = Rc::new(RefCell::new(None));
    if consumer_start > 5 {
        let p = probe.clone();
        let m = memory.clone();
        let c = clock.clone();
        engine.spawner().spawn(async move {
            c.wait_ticks(5)?.await;
            *p.borrow_mut() = m.service_snapshot();
            Ok(())
        });
    }
    engine.run().unwrap();
    let a = accepted.borrow().clone();
    let r = received.borrow().clone();
    let p = *probe.borrow();
    (
        a,
        r,
        memory.service_snapshot(),
        p,
        memory.bytes_read(),
        memory.bytes_written(),
    )
}
#[test]
fn late_arrival_cannot_spend_earlier_idle_capacity() {
    let (a, r, s, _, read, _) = run_requests(
        &[
            (0, 128, AccessType::ReadRequest),
            (10, 128, AccessType::ReadRequest),
        ],
        Some(2),
        3,
        0,
    );
    assert_eq!(a, vec![0, 10]);
    assert_eq!(r.iter().map(|x| x.3).collect::<Vec<_>>(), vec![4, 14]);
    assert_eq!(read, 256);
    assert_eq!(s.unwrap().total, 0);
}
#[test]
fn output_stall_retains_total_budget_at_zero_and_positive_latency() {
    for delay in [0, 3] {
        let inputs = vec![(0, 128, AccessType::ReadRequest); 4];
        let (a, r, s, p, read, _) = run_requests(&inputs, Some(2), delay, 10);
        assert_eq!(a, vec![0, 0, 10, 10]);
        assert_eq!(
            r.iter().map(|x| x.3).collect::<Vec<_>>(),
            vec![10, 10, 11 + delay as u64, 11 + delay as u64]
        );
        let p = p.unwrap();
        assert_eq!((p.queued, p.active, p.completed, p.total), (0, 0, 2, 2));
        let s = s.unwrap();
        assert_eq!(
            (
                s.total,
                s.peak_total,
                s.accepted_requests,
                s.retired_requests
            ),
            (0, 2, 4, 4)
        );
        assert_eq!(read, 512);
    }
}
#[test]
fn one_slot_budget_applies_backpressure_without_retrospective_service() {
    let (a, r, s, _, _, _) = run_requests(&[(0, 128, AccessType::ReadRequest); 4], Some(1), 3, 0);
    assert_eq!(a, vec![0, 4, 8, 12]);
    assert_eq!(
        r.iter().map(|x| x.3).collect::<Vec<_>>(),
        vec![4, 8, 12, 16]
    );
    assert_eq!(s.unwrap().peak_total, 1);
    assert_eq!(s.unwrap().total, 0);
}
#[test]
fn heterogeneous_read_and_nonposted_write_payloads_share_one_server() {
    let (a, r, s, _, read, write) = run_requests(
        &[
            (0, 128, AccessType::ReadRequest),
            (0, 3350, AccessType::WriteNonPostedRequest),
            (0, 7, AccessType::ReadRequest),
        ],
        Some(3),
        0,
        0,
    );
    assert_eq!(a, vec![0, 0, 0]);
    assert_eq!(r.iter().map(|x| x.3).collect::<Vec<_>>(), vec![1, 2, 2]);
    assert_eq!(r[1].2, AccessType::WriteNonPostedResponse);
    assert_eq!((read, write), (135, 3350));
    assert_eq!(s.unwrap().retired_requests, 3);
}
#[test]
fn posted_writes_retire_at_service_completion_without_response_latency() {
    let (a, r, s, _, read, write) =
        run_requests(&[(0, 128, AccessType::WriteRequest); 4], Some(1), 200, 0);
    assert_eq!(a, vec![0, 1, 2, 3]);
    assert!(r.is_empty());
    assert_eq!((read, write), (0, 512));
    assert_eq!((s.unwrap().total, s.unwrap().retired_requests), (0, 4));
}
#[test]
fn omitted_policy_retains_legacy_acceptance_and_response_timing() {
    let (a, r, s, _, read, write) =
        run_requests(&[(0, 128, AccessType::ReadRequest); 4], None, 3, 0);
    assert_eq!(a, vec![0, 1, 2, 3]);
    assert_eq!(r.iter().map(|x| x.3).collect::<Vec<_>>(), vec![3, 4, 5, 6]);
    assert!(s.is_none());
    assert_eq!((read, write), (512, 0));
}
#[test]
fn metadata_budget_and_actual_clock_are_checked_before_registration() {
    for budget in [0, 4097] {
        let mut e = Engine::default();
        let c = e.default_clock();
        let result = Memory::<MemoryAccess>::new_and_register(
            &e,
            &c,
            e.top(),
            "memory",
            MemoryConfig::new(0, 1, 1, 0).with_arrival_aware_service(budget),
        );
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("budget"));
    }
    let mut e = Engine::default();
    let c = e.clock_ghz(2.0);
    assert!(
        Memory::<MemoryAccess>::new_and_register(
            &e,
            &c,
            e.top(),
            "memory",
            MemoryConfig::new(0, 1, 1, 0).with_arrival_aware_service(1)
        )
        .is_err()
    );
    assert!(
        Memory::<MemoryAccess>::new_and_register(
            &e,
            &c,
            e.top(),
            "legacy",
            MemoryConfig::new(0, 1, 1, 0)
        )
        .is_ok()
    );
}
#[test]
fn invalid_ranges_access_kinds_and_deadlines_do_not_commit_owners_or_counters() {
    use gwr_engine::types::DeviceId;
    for (kind, bytes, address, delay, message) in [
        (AccessType::ReadRequest, 0, 0x80000, 0, "range"),
        (AccessType::ReadRequest, 2, u64::MAX, 0, "range"),
        (AccessType::ReadRequest, 1, 0, 0, "bounds"),
        (AccessType::Control, 1, 0x80000, 0, "unsupported"),
        (AccessType::ReadResponse, 1, 0x80000, 0, "unsupported"),
        (AccessType::ReadRequest, 1, 0x80000, usize::MAX, "deadline"),
    ] {
        let mut e = Engine::new(&gwr_track::tracker::dev_null_tracker());
        let c = e.default_clock();
        let memory = Memory::new_and_register(
            &e,
            &c,
            e.top(),
            "memory",
            MemoryConfig::new(0x80000, 65536, 1, delay).with_arrival_aware_service(1),
        )
        .unwrap();
        let packet = MemoryAccess::new(
            e.top(),
            kind,
            bytes,
            address,
            0x100,
            DeviceId(9),
            DeviceId(7),
            16,
        );
        let mut tx = OutPort::new(e.top(), "requests");
        tx.connect(memory.port_rx()).unwrap();
        let rx: InPort<MemoryAccess> = InPort::new(&e, &c, e.top(), "responses");
        memory.connect_port_tx(rx.state()).unwrap();
        let acknowledged = Rc::new(std::cell::Cell::new(false));
        let sender_acknowledged = acknowledged.clone();
        e.spawner().spawn(async move {
            tx.try_put()?.await;
            tx.put(packet)?.await;
            sender_acknowledged.set(true);
            Ok(())
        });
        let error = e.run().unwrap_err();
        assert!(
            !acknowledged.get(),
            "invalid request must not be acknowledged"
        );
        assert!(error.to_string().contains(message), "{error}");
        assert_eq!(
            memory.service_snapshot().unwrap(),
            gwr_models::memory::MemoryServiceSnapshot::default()
        );
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
    }
}
#[test]
fn one_descriptor_handles_large_payload_without_byte_derived_admission_rejection() {
    let (a, r, s, _, read, _) = run_requests(&[(0, 65536, AccessType::ReadRequest)], Some(1), 3, 0);
    assert_eq!(a, vec![0]);
    assert_eq!(r[0].3, 23);
    assert_eq!(read, 65536);
    assert_eq!((s.unwrap().total, s.unwrap().peak_total), (0, 1));
}

#[derive(Clone, Debug)]
struct RefuseResponse(MemoryAccess);
impl std::fmt::Display for RefuseResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl gwr_track::id::Unique for RefuseResponse {
    fn id(&self) -> gwr_track::Id {
        self.0.id()
    }
}
impl gwr_engine::traits::TotalBytes for RefuseResponse {
    fn total_bytes(&self) -> usize {
        self.0.total_bytes()
    }
}
impl gwr_engine::traits::SimObject for RefuseResponse {}
impl Routable for RefuseResponse {
    fn dst_addr(&self) -> u64 {
        self.0.dst_addr()
    }
    fn src_addr(&self) -> u64 {
        self.0.src_addr()
    }
    fn access_type(&self) -> AccessType {
        self.0.access_type()
    }
}
impl AccessMemory for RefuseResponse {
    fn to_ack(&self) -> Result<Self, gwr_engine::types::SimError> {
        self.0.to_ack().map(Self)
    }

    fn access_size_bytes(&self) -> usize {
        self.0.access_size_bytes()
    }
    fn to_response(
        &self,
        _: &impl gwr_models::memory::traits::ReadMemory,
    ) -> Result<Self, gwr_engine::types::SimError> {
        Err(gwr_engine::types::SimError(
            "custom response refused".into(),
        ))
    }
}
#[test]
fn custom_response_construction_failure_is_atomic_before_acceptance() {
    let mut e = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let c = e.default_clock();
    let memory = Memory::new_and_register(
        &e,
        &c,
        e.top(),
        "memory",
        MemoryConfig::new(0x80000, 65536, 3350, 0).with_arrival_aware_service(1),
    )
    .unwrap();
    let p = RefuseResponse(create_read(
        e.top(),
        &Rc::new(create_default_memory_map()),
        128,
        0x80000,
        0x81000,
        16,
    ));
    let mut tx = OutPort::new(e.top(), "requests");
    tx.connect(memory.port_rx()).unwrap();
    let rx: InPort<RefuseResponse> = InPort::new(&e, &c, e.top(), "responses");
    memory.connect_port_tx(rx.state()).unwrap();
    e.spawner().spawn(async move {
        tx.put(p)?.await;
        Ok(())
    });
    assert!(
        e.run()
            .unwrap_err()
            .to_string()
            .contains("custom response refused")
    );
    assert_eq!(
        memory.service_snapshot().unwrap(),
        gwr_models::memory::MemoryServiceSnapshot::default()
    );
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
}

#[test]
fn legitimate_inclusive_last_address_is_served_without_range_overflow() {
    use gwr_engine::types::DeviceId;
    let mut e = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let c = e.default_clock();
    let memory = Memory::new_and_register(
        &e,
        &c,
        e.top(),
        "memory",
        MemoryConfig::new(u64::MAX, 1, 3350, 0).with_arrival_aware_service(1),
    )
    .unwrap();
    let p = MemoryAccess::new(
        e.top(),
        AccessType::ReadRequest,
        1,
        u64::MAX,
        0x100,
        DeviceId(9),
        DeviceId(7),
        16,
    );
    let mut tx = OutPort::new(e.top(), "requests");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx: InPort<MemoryAccess> = InPort::new(&e, &c, e.top(), "responses");
    memory.connect_port_tx(rx.state()).unwrap();
    e.spawner().spawn(async move {
        tx.put(p)?.await;
        Ok(())
    });
    e.spawner().spawn(async move {
        let p = rx.get()?.await;
        assert_eq!(p.dst_addr(), u64::MAX);
        assert_eq!(p.access_size_bytes(), 1);
        Ok(())
    });
    e.run().unwrap();
    assert_eq!(memory.bytes_read(), 1);
    assert_eq!(memory.service_snapshot().unwrap().total, 0);
}

#[test]
fn failed_response_publication_does_not_release_accepted_owner() {
    let mut e = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let c = e.default_clock();
    let memory = Memory::new_and_register(
        &e,
        &c,
        e.top(),
        "memory",
        MemoryConfig::new(0x80000, 65536, 3350, 0).with_arrival_aware_service(1),
    )
    .unwrap();
    let p = create_read(
        e.top(),
        &Rc::new(create_default_memory_map()),
        128,
        0x80000,
        0x81000,
        16,
    );
    let mut tx = OutPort::new(e.top(), "requests");
    tx.connect(memory.port_rx()).unwrap();
    e.spawner().spawn(async move {
        tx.put(p)?.await;
        Ok(())
    });
    assert!(e.run().is_err());
    let s = memory.service_snapshot().unwrap();
    assert_eq!(
        (
            s.total,
            s.completed,
            s.accepted_requests,
            s.retired_requests
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(memory.bytes_read(), 128);
}

#[test]
fn read_response_byte_overflow_fails_before_owners_and_payload_counters() {
    use gwr_engine::types::DeviceId;
    let mut e = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let c = e.default_clock();
    let memory = Memory::new_and_register(
        &e,
        &c,
        e.top(),
        "memory",
        MemoryConfig::new(0x80000, 65536, 3350, 0).with_arrival_aware_service(1),
    )
    .unwrap();
    let p = MemoryAccess::new(
        e.top(),
        AccessType::ReadRequest,
        1,
        0x80000,
        0x100,
        DeviceId(9),
        DeviceId(7),
        usize::MAX,
    );
    let mut tx = OutPort::new(e.top(), "requests");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx: InPort<MemoryAccess> = InPort::new(&e, &c, e.top(), "responses");
    memory.connect_port_tx(rx.state()).unwrap();
    e.spawner().spawn(async move {
        tx.put(p)?.await;
        Ok(())
    });
    e.spawner().spawn(async move {
        rx.get()?.await;
        Ok(())
    });
    let error = e.run().unwrap_err();
    assert!(error.to_string().contains("response byte"), "{error}");
    assert_eq!(
        memory.service_snapshot().unwrap(),
        gwr_models::memory::MemoryServiceSnapshot::default()
    );
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
}

#[test]
fn buffered_admission_resumes_reads_posted_and_nonposted_requests() {
    use gwr_components::store::ByteStore;
    use gwr_models::test_helpers::{create_write, create_write_np};
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let memory = Memory::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "memory",
        MemoryConfig::new(0x80000, 65536, 3350, 3).with_arrival_aware_service(1),
    )
    .unwrap();
    let buffer =
        ByteStore::new_and_register(&engine, &clock, engine.top(), "request_buffer", 4096).unwrap();
    buffer.connect_port_tx(memory.port_rx()).unwrap();
    let mut tx = OutPort::new(engine.top(), "requests");
    tx.connect(buffer.port_rx()).unwrap();
    let mut rx: InPort<MemoryAccess> = InPort::new(&engine, &clock, engine.top(), "responses");
    memory.connect_port_tx(rx.state()).unwrap();
    let map = Rc::new(create_default_memory_map());
    let mut packets = vec![];
    for i in 0..3 {
        let address = 0x80000 + i * 384;
        packets.push(create_read(engine.top(), &map, 128, address, 0x81000, 16));
        packets.push(create_write(
            engine.top(),
            &map,
            128,
            address + 128,
            0x81000,
            16,
        ));
        packets.push(create_write_np(
            engine.top(),
            &map,
            128,
            address + 256,
            0x81000,
            16,
        ));
    }
    let accepted = Rc::new(RefCell::new(0));
    let accepted_log = accepted.clone();
    engine.spawn(async move {
        for p in packets {
            tx.put(p)?.await;
            *accepted_log.borrow_mut() += 1;
        }
        Ok(())
    });
    let replies = Rc::new(RefCell::new(vec![]));
    let result = replies.clone();
    engine.spawn(async move {
        for _ in 0..6 {
            let p = rx.get()?.await;
            result.borrow_mut().push((
                p.access_type(),
                p.access_size_bytes(),
                p.dst_addr(),
                p.src_addr(),
            ));
        }
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(*accepted.borrow(), 9);
    assert_eq!(replies.borrow().len(), 6);
    for (i, pair) in replies.borrow().as_chunks::<2>().0.iter().enumerate() {
        assert_eq!(
            pair[0],
            (
                AccessType::ReadResponse,
                128,
                0x80000 + i as u64 * 384,
                0x81000
            )
        );
        assert_eq!(
            pair[1],
            (
                AccessType::WriteNonPostedResponse,
                128,
                0x80100 + i as u64 * 384,
                0x81000
            )
        );
    }
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (384, 768));
    let s = memory.service_snapshot().unwrap();
    assert_eq!((s.accepted_requests, s.retired_requests), (9, 9));
    assert_eq!(
        (s.total, s.queued, s.active, s.completed, s.peak_total),
        (0, 0, 0, 0, 1)
    );
}
