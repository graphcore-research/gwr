// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::cell::Cell;

use gwr_engine::types::DeviceId;

use super::*;
use crate::cache::traits::CoherentAccess;
use crate::memory::memory_access::MemoryAccess;

mod ports;

const CACHE_DEVICE_ID: DeviceId = DeviceId(1);
const MANAGER_DEVICE_ID: DeviceId = DeviceId(3);
const REQUEST_HEADER_SIZE_BYTES: usize = 16;
const LINE_SIZE_BYTES: usize = 32;
const BANDWIDTH_BYTES_PER_TICK: usize = 8;
const EXPECTED_HEADER_SERVICE_TICKS: u64 =
    REQUEST_HEADER_SIZE_BYTES.div_ceil(BANDWIDTH_BYTES_PER_TICK) as u64;
const OUTPUT_STALL_TICKS: u64 = 20;
const OUTPUT_PACKET_COUNT: usize = 4;
const INVALIDATED_LINE_ADDRESS: u64 = 2 * (OUTPUT_PACKET_COUNT * LINE_SIZE_BYTES) as u64;
const MANAGER_MAP_SIZE_BYTES: u64 = 4096;

#[test]
fn writeback_and_ack_from_different_workers_keep_fifo_under_output_backpressure() {
    // Connect the control queue to a bandwidth-limited memory output.
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let state = create_coherent_cache_state(&engine, &clock);
    let (control_tx, mut memory_requests_rx) = create_memory_output_ports(&engine, &clock, &state);
    let state_clone = state.clone();
    engine.spawn(async move { run_control_tx(state_clone, control_tx).await });

    // Submit a writeback and an acknowledgement from separate workers.
    let writeback = create_memory_request(&engine, AccessType::WriteRequest, 0, LINE_SIZE_BYTES)
        .with_coherence_op(Some(CoherenceOp::Writeback));
    let invalidation_ack = writeback
        .clone()
        .with_access_type(AccessType::Control)
        .with_coherence_op(Some(CoherenceOp::InvalidateAck));

    let state_clone = state.clone();
    engine.spawn(async move { send_control(&state_clone, writeback).await });

    let state_clone = state.clone();
    let clock_clone = clock.clone();
    engine.spawn(async move {
        clock_clone.wait_ticks(1)?.await;
        send_control(&state_clone, invalidation_ack).await
    });

    // Check the acknowledgement before its header service completes.
    let state_clone = state.clone();
    let clock_clone = clock.clone();
    engine.spawn(async move {
        clock_clone.wait_ticks(OUTPUT_STALL_TICKS + 1)?.await;
        assert_eq!(
            state_clone.completed_control_count.get(),
            1,
            "delivery must precede completion of acknowledgement service"
        );
        Ok(())
    });

    // Block memory output, then check FIFO delivery.
    let received_all_control_packets = Rc::new(Cell::new(false));
    let received_all_control_packets_clone = received_all_control_packets.clone();
    let state_clone = state.clone();
    engine.spawn(async move {
        clock.wait_ticks(OUTPUT_STALL_TICKS)?.await;
        assert_eq!(state_clone.completed_control_count.get(), 0);
        assert_eq!(
            memory_requests_rx.get()?.await.coherence_op(),
            Some(CoherenceOp::Writeback)
        );
        assert_eq!(
            memory_requests_rx.get()?.await.coherence_op(),
            Some(CoherenceOp::InvalidateAck)
        );
        received_all_control_packets_clone.set(true);
        Ok(())
    });

    gwr_engine::run_simulation!(engine);

    assert!(received_all_control_packets.get());
    assert!(state.pending_control_packets.borrow().is_empty());
    assert_eq!(state.completed_control_count.get(), 2);
}

#[test]
fn ordinary_requests_do_not_complete_control_tickets() {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let state = create_coherent_cache_state(&engine, &clock);
    let (mut selected_requests_tx, mut memory_requests_rx) =
        create_memory_output_ports(&engine, &clock, &state);

    // Control access alone must not identify a queued control packet.
    let read_request = create_memory_request(&engine, AccessType::ReadRequest, 0, LINE_SIZE_BYTES);
    let shared_read_request = read_request
        .clone()
        .with_coherence_op(Some(CoherenceOp::SharedRead));
    let exclusive_write_request = read_request
        .clone()
        .with_access_type(AccessType::Control)
        .with_coherence_op(Some(CoherenceOp::ExclusiveWrite));
    engine.spawn(async move {
        selected_requests_tx.put(read_request)?.await;
        selected_requests_tx.put(shared_read_request)?.await;
        selected_requests_tx.put(exclusive_write_request)?.await;
        Ok(())
    });

    // Observe delivery of every request before checking the completion count.
    let received_request_count = Rc::new(Cell::new(0));
    let received_request_count_clone = received_request_count.clone();
    engine.spawn(async move {
        for _ in 0..3 {
            memory_requests_rx.get()?.await;
            received_request_count_clone.set(received_request_count_clone.get() + 1);
        }
        Ok(())
    });

    gwr_engine::run_simulation!(engine);

    assert_eq!(received_request_count.get(), 3);
    assert_eq!(state.completed_control_count.get(), 0);
}

#[test]
fn fill_requests_and_invalidation_acks_share_memory_output_bandwidth() {
    for request_delay_ticks in [0, 3] {
        let request_arrivals = memory_output_arrivals(0, true, request_delay_ticks);

        // Check the mixed traffic count and initial output latency.
        assert_eq!(request_arrivals.len(), OUTPUT_PACKET_COUNT);
        assert_eq!(
            request_arrivals[0].0,
            EXPECTED_HEADER_SERVICE_TICKS + request_delay_ticks as u64
        );
        assert_eq!(
            request_arrivals
                .iter()
                .filter(|(_, coherence_op)| *coherence_op == CoherenceOp::SharedRead)
                .count(),
            OUTPUT_PACKET_COUNT - 1,
        );
        assert_eq!(
            request_arrivals
                .iter()
                .filter(|(_, coherence_op)| *coherence_op == CoherenceOp::InvalidateAck)
                .count(),
            1,
        );

        // Each ordinary or control packet must share the output service
        // interval.
        assert!(
            request_arrivals
                .windows(2)
                .all(
                    |adjacent_arrivals| adjacent_arrivals[1].0 - adjacent_arrivals[0].0
                        >= EXPECTED_HEADER_SERVICE_TICKS
                ),
            "each {REQUEST_HEADER_SIZE_BYTES}-byte packet must share the {BANDWIDTH_BYTES_PER_TICK}-byte/tick output: {request_arrivals:?}"
        );
    }
}

#[test]
fn memory_output_backpressure_overlaps_packet_service_without_extra_delay() {
    for request_delay_ticks in [0, 3] {
        let request_arrivals =
            memory_output_arrivals(OUTPUT_STALL_TICKS, false, request_delay_ticks);
        let request_arrival_ticks: Vec<_> =
            request_arrivals.into_iter().map(|(tick, _)| tick).collect();

        // The first packet's service has elapsed while its handoff was blocked.
        // The next packet can start immediately; subsequent packets remain
        // paced.
        assert_eq!(
            request_arrival_ticks,
            [
                OUTPUT_STALL_TICKS,
                OUTPUT_STALL_TICKS,
                OUTPUT_STALL_TICKS + EXPECTED_HEADER_SERVICE_TICKS,
                OUTPUT_STALL_TICKS + 2 * EXPECTED_HEADER_SERVICE_TICKS,
            ]
        );
    }
}

fn create_coherent_cache_state(engine: &Engine, clock: &Clock) -> CacheState<MemoryAccess> {
    let mut contents = CacheContents::new(
        engine.top(),
        CacheConfig::new(LINE_SIZE_BYTES, BANDWIDTH_BYTES_PER_TICK, 1, 1, 0),
    );
    contents.coherence = Some(Rc::new(coherence::DirectoryCoherenceProtocol(
        create_coherence_config(),
    )));

    CacheState {
        entity: engine.top().clone(),
        clock: clock.clone(),
        contents: Rc::new(RefCell::new(contents)),
        metrics: Rc::new(RefCell::new(CacheMetrics::default())),
        state_changed: Repeated::default(),
        bandwidth_bytes_per_tick: BANDWIDTH_BYTES_PER_TICK,
        pending_parent_returns: Rc::new(RefCell::new(VecDeque::new())),
        pending_control_packets: Rc::new(RefCell::new(VecDeque::new())),
        submitted_control_count: Rc::new(Cell::new(0)),
        completed_control_count: Rc::new(Cell::new(0)),
    }
}

fn create_memory_output_ports(
    engine: &Engine,
    clock: &Clock,
    state: &CacheState<MemoryAccess>,
) -> (OutPort<MemoryAccess>, InPort<MemoryAccess>) {
    let mut selected_requests_tx = OutPort::new(engine.top(), "selected_requests_tx");
    let selected_requests_rx = InPort::new(engine, clock, engine.top(), "selected_requests_rx");
    selected_requests_tx
        .connect(selected_requests_rx.state())
        .unwrap();

    let mut memory_requests_tx = OutPort::new(engine.top(), "memory_requests_tx");
    let memory_requests_rx = InPort::new(engine, clock, engine.top(), "memory_requests_rx");
    let limiter = Limiter::new_and_register(
        engine,
        clock,
        engine.top(),
        "memory_output_limiter",
        Rc::new(RateLimiter::new(clock, state.bandwidth_bytes_per_tick * 8).unwrap()),
    );
    memory_requests_tx.connect(limiter.port_rx()).unwrap();
    limiter.connect_port_tx(memory_requests_rx.state()).unwrap();

    let state_clone = state.clone();
    engine.spawn(async move {
        forward_memory_requests(selected_requests_rx, state_clone, memory_requests_tx).await
    });

    (selected_requests_tx, memory_requests_rx)
}

fn memory_output_arrivals(
    initial_stall_ticks: u64,
    include_invalidation: bool,
    request_delay_ticks: usize,
) -> Vec<(u64, CoherenceOp)> {
    use crate::cache::traits::CoherenceToken;

    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let cache = Cache::new_coherent_and_register(
        &engine,
        &clock,
        engine.top(),
        "cache",
        CacheConfig::new(
            LINE_SIZE_BYTES,
            BANDWIDTH_BYTES_PER_TICK,
            8,
            1,
            request_delay_ticks,
        ),
        create_coherence_config(),
    )
    .unwrap();
    let mut memory_requests_rx = InPort::new(&engine, &clock, engine.top(), "memory_requests_rx");
    cache
        .connect_port_mem_tx(memory_requests_rx.state())
        .unwrap();
    let mut device_requests_tx = OutPort::new(engine.top(), "device_requests_tx");
    device_requests_tx.connect(cache.port_dev_rx()).unwrap();

    // Drive a multiline read to produce ordinary fill requests.
    let read_request = create_memory_request(
        &engine,
        AccessType::ReadRequest,
        0,
        (OUTPUT_PACKET_COUNT - usize::from(include_invalidation)) * LINE_SIZE_BYTES,
    );
    engine.spawn(async move {
        device_requests_tx.put(read_request)?.await;
        Ok(())
    });

    // Add unrelated invalidation traffic when testing shared bandwidth.
    let mut manager_requests_tx = OutPort::new(engine.top(), "manager_requests_tx");
    manager_requests_tx.connect(cache.port_mem_rx()).unwrap();
    if include_invalidation {
        let invalidation_request = create_memory_request(
            &engine,
            AccessType::Control,
            INVALIDATED_LINE_ADDRESS,
            LINE_SIZE_BYTES,
        )
        .with_routing(CACHE_DEVICE_ID, MANAGER_DEVICE_ID)
        .with_coherence_op(Some(CoherenceOp::Invalidate))
        .with_coherence_token(Some(CoherenceToken {
            owner: Rc::new(()),
            ordinal: 0,
        }));
        engine.spawn(async move {
            manager_requests_tx.put(invalidation_request)?.await;
            Ok(())
        });
    }

    // Record output arrivals after any downstream stall.
    let request_arrivals = Rc::new(RefCell::new(Vec::new()));
    let request_arrivals_clone = request_arrivals.clone();
    engine.spawn(async move {
        clock.wait_ticks(initial_stall_ticks)?.await;
        for _ in 0..OUTPUT_PACKET_COUNT {
            let memory_request = memory_requests_rx.get()?.await;
            request_arrivals_clone.borrow_mut().push((
                clock.tick_now().tick(),
                memory_request.coherence_op().unwrap(),
            ));
        }
        Ok(())
    });

    gwr_engine::run_simulation!(engine);

    Rc::try_unwrap(request_arrivals).unwrap().into_inner()
}

fn create_coherence_config() -> CoherenceConfig {
    let mut manager_map = crate::memory::memory_map::MemoryMap::new();
    manager_map
        .insert(0, MANAGER_MAP_SIZE_BYTES, MANAGER_DEVICE_ID)
        .unwrap();
    CoherenceConfig::new(CACHE_DEVICE_ID, Rc::new(manager_map))
}

/// Default to cache-to-manager routing and a header matching the timing
/// fixtures.
fn create_memory_request(
    engine: &Engine,
    access_type: AccessType,
    address: u64,
    access_size_bytes: usize,
) -> MemoryAccess {
    MemoryAccess::new(
        engine.top(),
        access_type,
        access_size_bytes,
        address,
        0,
        MANAGER_DEVICE_ID,
        CACHE_DEVICE_ID,
        REQUEST_HEADER_SIZE_BYTES,
    )
}
