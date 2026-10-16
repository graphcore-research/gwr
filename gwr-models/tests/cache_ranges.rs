// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Native cache request-range correctness, independent of hardware calibration.
use std::rc::Rc;

use gwr_components::connect_port;
use gwr_engine::engine::Engine;
use gwr_models::build_model_harness;
use gwr_models::memory::cache::{Cache, CacheConfig};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{
    MemoryTxn, create_default_memory_map, create_read, create_write, create_write_np,
};
use gwr_track::entity::GetEntity;
build_model_harness! {
    harness RangeHarness<T: gwr_models::memory::traits::CacheAccess> {
        component:cache:Rc<Cache<T>>,
        rx ports:{DevRx<T> => dev_rx},
        tx ports:{DevTx<T> => dev_tx},
    }
}

#[test]
fn partial_fill_does_not_fabricate_unfetched_line_data() {
    let (mut harness, cache, memory, _) = hierarchy(CacheConfig::new(32, 32, 16, 2, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let first = create_read(cache.entity(), &map, 1, 0x80000, 0x81000, 16);
    let other = create_read(cache.entity(), &map, 1, 0x8001f, 0x81000, 16);
    harness.run_steps([
        send_dev_rx!(first),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(1)),
        send_dev_rx!(other),
        expect_dev_tx!(MemoryTxn::read_rsp(0x8001f).with_bytes(1)),
    ]);
    assert_eq!(
        memory.bytes_read(),
        32,
        "one actual full validity-unit fill is needed"
    );
    assert_eq!(cache.payload_bytes_read(), 2);
    assert_eq!(cache.num_misses(), 1);
    assert_eq!(cache.num_hits(), 1);
}

#[test]
fn unaligned_multiline_read_populates_every_real_validity_unit() {
    let (mut harness, cache, memory, _) = hierarchy(CacheConfig::new(32, 32, 16, 2, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let first = create_read(cache.entity(), &map, 96, 0x80001, 0x81000, 16);
    let mut steps = vec![
        send_dev_rx!(first),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80001).with_bytes(96)),
    ];
    // Probe each fetched line, including bytes outside the original request
    // at both ends of the rounded fill range.
    for address in [0x80000, 0x80020, 0x80040, 0x8007f] {
        let probe = create_read(cache.entity(), &map, 1, address, 0x81000, 16);
        steps.push(send_dev_rx!(probe));
        steps.push(expect_dev_tx!(MemoryTxn::read_rsp(address).with_bytes(1)));
    }
    harness.run_steps(steps);

    assert_eq!(memory.bytes_read(), 128);
    assert_eq!(cache.num_misses(), 1);
    assert_eq!(cache.num_hits(), 4);
    assert_eq!(cache.traffic_snapshot().fill_requests, 4);
}

#[test]
fn nested_caches_translate_different_line_sizes_and_restore_client_ranges() {
    for (upper_bytes, lower_bytes, request_bytes, expected_bytes) in
        [(32, 128, 64, 128), (128, 32, 32, 128)]
    {
        let (mut harness, upper, memory, lower) = hierarchy(
            CacheConfig::new(upper_bytes, 32, 16, 2, 1),
            Some(CacheConfig::new(lower_bytes, 32, 16, 2, 1)),
            None,
        );
        let map = Rc::new(create_default_memory_map());
        let read = create_read(upper.entity(), &map, request_bytes, 0x80000, 0x81000, 16);
        harness.run_steps([
            send_dev_rx!(read.clone()),
            expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(request_bytes)),
            send_dev_rx!(read),
            expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(request_bytes)),
        ]);
        assert_eq!(memory.bytes_read(), expected_bytes);
        assert_eq!(upper.num_misses(), 1);
        assert_eq!(upper.num_hits(), 1);
        assert_eq!(upper.inflight_fills_snapshot().current, 0);
        assert_eq!(lower.unwrap().inflight_fills_snapshot().current, 0);
    }
}

#[test]
fn line_validity_noallocate_writes_and_headers_are_physical() {
    use gwr_models::memory::CacheHintType;
    let (mut harness, cache, memory, _) =
        hierarchy(CacheConfig::new(128, 32, 16, 2, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let read = |a| create_read(cache.entity(), &map, 1, a, 0x81000, 16);
    let noalloc = read(0x80160).with_cache_hint(CacheHintType::NoAllocate);
    let write = create_write(cache.entity(), &map, 1, 0x80000, 0x81000, 16);
    harness.run_steps([
        send_dev_rx!(read(0x80000)),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(1)),
        send_dev_rx!(read(0x80020)),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80020).with_bytes(1)),
        send_dev_rx!(noalloc.clone()),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80160).with_bytes(1)),
        send_dev_rx!(noalloc),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80160).with_bytes(1)),
        send_dev_rx!(write),
        send_dev_rx!(read(0x80020)),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80020).with_bytes(1)),
        send_dev_rx!(read(0x80000)),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(1)),
    ]);
    assert_eq!(memory.bytes_read(), 512);
    assert_eq!(memory.bytes_written(), 1);
    let traffic = cache.traffic_snapshot();
    assert_eq!(traffic.logical_read_bytes, 6);
    assert_eq!(traffic.request_hits, 2);
    assert_eq!(traffic.request_misses, 4);
    assert_eq!(traffic.fill_requests, 4);
    assert_eq!(traffic.downstream_request_header_bytes, 80);
    assert_eq!(traffic.downstream_response_header_bytes, 64);
}

#[test]
fn large_requests_stream_through_one_fill_and_parent_slot_with_nested_null_tracing() {
    let (mut harness, cache, memory, _) = hierarchy(
        CacheConfig::new(128, 32, 128, 2, 1).with_admission_limits(1, 1),
        Some(CacheConfig::new(128, 32, 128, 2, 1).with_admission_limits(1, 1)),
        None,
    );
    let map = Rc::new(create_default_memory_map());
    let read = create_read(cache.entity(), &map, 4096, 0x80000, 0x81000, 16);
    harness.run_steps([
        send_dev_rx!(read.clone()),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(4096)),
        send_dev_rx!(read),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(4096)),
    ]);
    assert_eq!(memory.bytes_read(), 4096);
    let traffic = cache.traffic_snapshot();
    assert_eq!(traffic.fill_requests, 32);
    assert_eq!(traffic.request_hits, 1);
    assert_eq!(traffic.request_misses, 1);
    assert_eq!(traffic.line_misses, 32);
    assert_eq!(traffic.line_hits, 32);
    assert_eq!(traffic.downstream_request_header_bytes, 32 * 16);
    assert_eq!(traffic.downstream_response_header_bytes, 32 * 16);
}

#[test]
fn overlapping_pending_lines_coalesce_once_and_complete_each_original() {
    let (mut harness, cache, memory, _) =
        hierarchy(CacheConfig::new(128, 32, 16, 2, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let first = create_read(cache.entity(), &map, 64, 0x80000, 0x81000, 16);
    let overlap = create_read(cache.entity(), &map, 64, 0x80020, 0x81000, 16);
    harness.run_steps([par!([
        seq!([send_dev_rx!(first), send_dev_rx!(overlap)]),
        seq!([
            expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(64)),
            expect_dev_tx!(MemoryTxn::read_rsp(0x80020).with_bytes(64))
        ]),
    ])]);
    assert_eq!(memory.bytes_read(), 128);
    let traffic = cache.traffic_snapshot();
    assert_eq!(traffic.fill_requests, 1);
    assert_eq!(traffic.line_misses, 1);
    assert_eq!(traffic.coalesced_lines, 1);
    assert_eq!(traffic.request_misses, 2);
    assert_eq!(traffic.request_hits, 0);
}

#[test]
fn write_invalidates_a_whole_pending_line() {
    let (mut harness, cache, memory, _) =
        hierarchy(CacheConfig::new(128, 32, 16, 2, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let first = create_read(cache.entity(), &map, 128, 0x80000, 0x81000, 16);
    let write = create_write(cache.entity(), &map, 1, 0x80000, 0x81000, 16);
    let reread = create_read(cache.entity(), &map, 1, 0x80000, 0x81000, 16);
    let unaffected = create_read(cache.entity(), &map, 1, 0x80020, 0x81000, 16);
    harness.run_steps([
        par!([
            seq!([send_dev_rx!(first), send_dev_rx!(write)]),
            expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(128))
        ]),
        send_dev_rx!(reread),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(1)),
        send_dev_rx!(unaffected),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80020).with_bytes(1)),
    ]);
    assert_eq!(memory.bytes_read(), 256);
    assert_eq!(memory.bytes_written(), 1);
    let traffic = cache.traffic_snapshot();
    assert_eq!(traffic.fill_requests, 2);
    assert_eq!(traffic.request_hits, 1);
}

#[test]
fn concurrent_replacement_does_not_install_an_obsolete_tag_response() {
    let (mut harness, cache, memory, _) = hierarchy(CacheConfig::new(128, 32, 1, 1, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let a = create_read(cache.entity(), &map, 1, 0x80000, 0x81000, 16);
    let b = create_read(cache.entity(), &map, 1, 0x80080, 0x81000, 16);
    harness.run_steps([
        par!([
            seq!([send_dev_rx!(a.clone()), send_dev_rx!(b)]),
            seq!([
                expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(1)),
                expect_dev_tx!(MemoryTxn::read_rsp(0x80080).with_bytes(1))
            ])
        ]),
        send_dev_rx!(a),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(1)),
    ]);
    assert_eq!(memory.bytes_read(), 384);
    assert_eq!(cache.traffic_snapshot().fill_requests, 3);
}

#[test]
fn parent_completion_waits_for_full_rounded_fill_ingress_service() {
    let fast = read_completion_ticks(32);
    let slow = read_completion_ticks(1);
    // A 128-byte fill plus its 16-byte header takes 144 ingress ticks;
    // the original one-byte response plus header then takes 17 return ticks.
    assert!(slow >= 161);
    assert!(
        slow >= fast + 150,
        "slow{slow} fast{fast}: final fill service was exposed too early"
    );
}

#[test]
fn final_physical_byte_can_be_read() {
    let (mut harness, cache, memory, _) = hierarchy(
        CacheConfig::new(128, 32, 16, 2, 1),
        None,
        Some(MemoryConfig::new(u64::MAX - 127, 128, 32, 2)),
    );
    let map = Rc::new(create_default_memory_map());
    let read = create_read(cache.entity(), &map, 1, u64::MAX, 0, 16);
    harness.run_steps([
        send_dev_rx!(read),
        expect_dev_tx!(MemoryTxn::read_rsp(u64::MAX).with_bytes(1)),
    ]);
    assert_eq!(memory.bytes_read(), 128);
}

#[test]
fn invalid_admission_limits_and_cache_geometry_are_rejected() {
    for config in [
        CacheConfig::new(128, 32, 1, 1, 1).with_admission_limits(0, 1),
        CacheConfig::new(usize::MAX, 32, 2, 2, 1),
    ] {
        assert!(config.validate().is_err());
    }
}

#[test]
#[should_panic(expected = "cache address range overflows")]
fn overflow_rejects_before_any_fill() {
    use gwr_engine::types::{AccessType, DeviceId};
    let (mut harness, cache, _, _) = hierarchy(CacheConfig::new(128, 32, 16, 2, 1), None, None);
    let read = MemoryAccess::new(
        cache.entity(),
        AccessType::ReadRequest,
        2,
        u64::MAX,
        0,
        DeviceId(0),
        DeviceId(1),
        16,
    );
    harness.run_steps([
        send_dev_rx!(read),
        expect_dev_tx!(MemoryTxn::read_rsp(u64::MAX).with_bytes(2)),
    ]);
}

#[test]
fn unaligned_multiline_nonposted_write_preserves_ack_and_payload() {
    let (mut harness, cache, memory, _) = hierarchy(CacheConfig::new(32, 32, 16, 2, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let read = create_read(cache.entity(), &map, 128, 0x80000, 0x81000, 16);
    let write = create_write_np(cache.entity(), &map, 34, 0x8001f, 0x81000, 16);
    harness.run_steps([
        send_dev_rx!(read.clone()),
        expect_dev_tx!(
            MemoryTxn::read_rsp(0x80000)
                .with_src_addr(0x81000)
                .with_bytes(128)
        ),
        send_dev_rx!(write),
        expect_dev_tx!(
            MemoryTxn::write_np_rsp(0x8001f)
                .with_src_addr(0x81000)
                .with_bytes(34)
        ),
        send_dev_rx!(read),
        expect_dev_tx!(
            MemoryTxn::read_rsp(0x80000)
                .with_src_addr(0x81000)
                .with_bytes(128)
        ),
    ]);
    assert_eq!(memory.bytes_read(), 224);
    assert_eq!(memory.bytes_written(), 34);
    let traffic = cache.traffic_snapshot();
    assert_eq!(traffic.fill_requests, 7);
    assert_eq!(traffic.line_hits, 1);
    assert_eq!(traffic.line_misses, 7);
    assert_eq!(traffic.downstream_write_bytes, 34);
    assert_eq!(traffic.downstream_request_header_bytes, 128);
    assert_eq!(traffic.downstream_response_header_bytes, 128);
}

#[test]
fn control_invalidates_pending_line_and_noallocate_can_hit_valid_data() {
    use gwr_engine::types::{AccessType, DeviceId};
    use gwr_models::memory::CacheHintType;
    let (mut harness, cache, memory, _) =
        hierarchy(CacheConfig::new(128, 32, 16, 2, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let read = create_read(cache.entity(), &map, 128, 0x80000, 0x81000, 16);
    let control = MemoryAccess::new(
        cache.entity(),
        AccessType::Control,
        0,
        0x80010,
        0x81000,
        DeviceId(0),
        DeviceId(1),
        16,
    );
    let noalloc = create_read(cache.entity(), &map, 1, 0x80020, 0x81000, 16)
        .with_cache_hint(CacheHintType::NoAllocate);
    harness.run_steps([
        par!([
            seq!([send_dev_rx!(read.clone()), send_dev_rx!(control)]),
            expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(128))
        ]),
        send_dev_rx!(read),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(128)),
        send_dev_rx!(noalloc),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80020).with_bytes(1)),
    ]);
    assert_eq!(memory.bytes_read(), 256);
    assert_eq!(cache.traffic_snapshot().request_hits, 1);
}

#[test]
fn one_parent_covers_valid_pending_and_missing_lines() {
    let (mut harness, cache, memory, _) = hierarchy(CacheConfig::new(32, 32, 16, 2, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let read = |address, bytes| create_read(cache.entity(), &map, bytes, address, 0x81000, 16);
    harness.run_steps([
        send_dev_rx!(read(0x80000, 1)),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(1)),
        par!([
            seq!([
                send_dev_rx!(read(0x80020, 1)),
                send_dev_rx!(read(0x80000, 96))
            ]),
            seq!([
                expect_dev_tx!(MemoryTxn::read_rsp(0x80020).with_bytes(1)),
                expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(96))
            ])
        ]),
    ]);
    let traffic = cache.traffic_snapshot();
    assert_eq!(memory.bytes_read(), 96);
    assert_eq!(traffic.fill_requests, 3);
    assert_eq!(traffic.line_hits, 1);
    assert_eq!(traffic.line_misses, 3);
    assert_eq!(traffic.coalesced_lines, 1);
    assert_eq!(traffic.downstream_request_header_bytes, 48);
    assert_eq!(traffic.downstream_response_header_bytes, 48);
}

#[test]
fn every_physical_fill_charges_request_header_bandwidth() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let cache = Cache::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "cache",
        CacheConfig::new(32, 8, 4, 1, 0),
    )
    .unwrap();
    let mut harness = RangeHarness::new(engine, cache.clone());
    let arrivals = serve_memory_fills(&mut harness, 3, 0);
    let map = Rc::new(create_default_memory_map());
    let read = create_read(cache.entity(), &map, 96, 0x80000, 0x81000, 16);

    harness.run_steps([
        send_dev_rx!(read),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(96)),
    ]);

    let arrivals = arrivals.borrow();
    assert_eq!(arrivals.len(), 3);
    assert!(
        arrivals.windows(2).all(|pair| pair[1] - pair[0] >= 2),
        "each 16-byte header needs two ticks at 8 bytes/tick: {arrivals:?}"
    );
    assert_eq!(cache.traffic_snapshot().downstream_request_header_bytes, 48);
}

#[test]
fn coalesced_reads_charge_client_request_headers() {
    use std::cell::RefCell;

    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let cache = Cache::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "cache",
        CacheConfig::new(32, 8, 4, 1, 0),
    )
    .unwrap();
    let mut harness = RangeHarness::new(engine, cache.clone());
    serve_memory_fills(&mut harness, 1, 100);
    let map = Rc::new(create_default_memory_map());
    let read = create_read(cache.entity(), &map, 1, 0x80000, 0x81000, 16);
    let acknowledgements = Rc::new(RefCell::new(Vec::new()));
    let observed = acknowledgements.clone();
    let send_and_record = script! {
        ports { requests = DevRx }
        async {
            requests.put(read.clone())?.await;
            observed.borrow_mut().push(clock.tick_now().tick());
            Ok(())
        }
    };

    let response = MemoryTxn::read_rsp(0x80000).with_bytes(1);
    harness.run_steps([par!([
        seq!(vec![send_and_record; 4]),
        seq!(vec![expect_dev_tx!(response); 4]),
    ])]);

    let acknowledgements = acknowledgements.borrow();
    assert_eq!(acknowledgements.len(), 4);
    assert!(
        acknowledgements
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= 2),
        "each client header needs two ticks even without a new fill: {acknowledgements:?}"
    );
    assert_eq!(cache.traffic_snapshot().fill_requests, 1);
    assert_eq!(cache.traffic_snapshot().coalesced_lines, 3);
}

fn serve_memory_fills(
    harness: &mut RangeHarness<MemoryAccess>,
    num_fills: usize,
    response_delay_ticks: u64,
) -> Rc<std::cell::RefCell<Vec<u64>>> {
    use std::cell::RefCell;

    use gwr_engine::port::{InPort, OutPort};
    use gwr_models::memory::traits::AccessMemory;

    let mut requests = InPort::new(
        &harness.engine,
        &harness.clock,
        harness.engine.top(),
        "requests",
    );
    harness.cache.connect_port_mem_tx(requests.state()).unwrap();
    let mut replies = OutPort::new(harness.engine.top(), "replies");
    replies.connect(harness.cache.port_mem_rx()).unwrap();
    let arrivals = Rc::new(RefCell::new(Vec::new()));
    let observed = arrivals.clone();
    let clock = harness.clock.clone();
    harness.engine.spawn(async move {
        // Receive every fill before replying so response service and backend
        // latency cannot account for the measured request spacing.
        let mut fills = Vec::new();
        for _ in 0..num_fills {
            fills.push(requests.get()?.await);
            observed.borrow_mut().push(clock.tick_now().tick());
        }
        clock.wait_ticks(response_delay_ticks)?.await;
        for fill in fills {
            replies.put(fill.to_response(&ReplyMemory)?)?.await;
        }
        Ok(())
    });
    arrivals
}

struct ReplyMemory;

impl gwr_models::memory::traits::ReadMemory for ReplyMemory {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn read_completion_ticks(bandwidth: usize) -> u64 {
    let (mut harness, cache, _, _) =
        hierarchy(CacheConfig::new(128, bandwidth, 16, 2, 1), None, None);
    let map = Rc::new(create_default_memory_map());
    let read = create_read(cache.entity(), &map, 1, 0x80000, 0x81000, 16);
    harness.run_steps([
        send_dev_rx!(read),
        expect_dev_tx!(MemoryTxn::read_rsp(0x80000).with_bytes(1)),
    ]);
    harness.clock.tick_now().tick()
}

fn hierarchy(
    config: CacheConfig,
    lower_config: Option<CacheConfig>,
    memory_config: Option<MemoryConfig>,
) -> CacheHierarchy {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let cache = Cache::new_and_register(&engine, &clock, engine.top(), "cache", config).unwrap();
    let memory = Memory::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "memory",
        memory_config.unwrap_or_else(|| MemoryConfig::new(0x80000, 65536, 32, 20)),
    )
    .unwrap();
    let lower = if let Some(config) = lower_config {
        let lower =
            Cache::new_and_register(&engine, &clock, engine.top(), "lower", config).unwrap();
        connect_port!(cache, mem_tx => lower, dev_rx).unwrap();
        connect_port!(lower, dev_tx => cache, mem_rx).unwrap();
        connect_port!(lower, mem_tx => memory, rx).unwrap();
        connect_port!(memory, tx => lower, mem_rx).unwrap();
        Some(lower)
    } else {
        connect_port!(cache, mem_tx => memory, rx).unwrap();
        connect_port!(memory, tx => cache, mem_rx).unwrap();
        None
    };
    (
        RangeHarness::new(engine, cache.clone()),
        cache,
        memory,
        lower,
    )
}

type CacheHierarchy = (
    RangeHarness<MemoryAccess>,
    Rc<Cache<MemoryAccess>>,
    Rc<Memory<MemoryAccess>>,
    Option<Rc<Cache<MemoryAccess>>>,
);
