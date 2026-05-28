// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Direct tests cover rejection atomicity: invalid input must leave in-flight
//! state usable. Port-level errors stop the simulation, so they cannot check
//! recovery with a subsequent valid response. Behavioral tests live in
//! cache::tests::ports and the cache integration tests.
use gwr_engine::engine::Engine;
use gwr_engine::types::{AccessType, DeviceId};

use super::*;
use crate::cache::coherence::{CoherenceConfig, DirectoryCoherenceProtocol};
use crate::cache::coherency_manager::CoherenceOp;
use crate::cache::traits::CoherentAccess;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::traits::{AccessMemory, ReadMemory};
use crate::test_helpers::{create_default_memory_map, create_read};

const CACHE_DEVICE_ID: DeviceId = DeviceId(1);
const DIRECTORY_DEVICE_ID: DeviceId = DeviceId(3);
const BACKING_MEMORY_DEVICE_ID: DeviceId = DeviceId(4);
const CLIENT_DEVICE_ID: DeviceId = DeviceId(5);
const COHERENT_LINE_SIZE_BYTES: usize = 32;
const COHERENT_BANDWIDTH_BYTES_PER_TICK: usize = 8;
const REQUEST_HEADER_SIZE_BYTES: usize = 16;
const MANAGER_MAP_SIZE_BYTES: u64 = 4096;

#[test]
fn read_admission_rejects_other_access_types_without_changing_state() {
    // Record the empty cache state.
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let mut contents = CacheContents::new(engine.top(), create_single_line_cache_config());
    let expected_inflight_fills = contents.inflight_fills_snapshot();

    // Check each unsupported access type.
    for access_type in [
        AccessType::Control,
        AccessType::WriteRequest,
        AccessType::WriteNonPostedRequest,
        AccessType::ReadResponse,
        AccessType::WriteNonPostedResponse,
    ] {
        let request = MemoryAccess::new(
            engine.top(),
            access_type,
            128,
            0x80000,
            0x81000,
            DeviceId(0),
            DeviceId(0),
            16,
        );

        // Reject the request without admitting a parent or reserving a fill.
        assert!(contents.admit_request(request).is_err(), "{access_type:?}");
        assert!(contents.parents.is_empty());
        assert_eq!(contents.next_parent_id_ordinal, 0);
        assert_eq!(contents.inflight_fills_snapshot(), expected_inflight_fills);
    }
}

#[test]
fn bad_responses_leave_inflight_fills_unchanged() {
    // Issue a fill and record its occupied slot.
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let memory_map = Rc::new(create_default_memory_map());
    let mut contents = CacheContents::new(engine.top(), create_single_line_cache_config());
    let (parent_id, line_start, request_end) = contents
        .admit_request(create_read(
            engine.top(),
            &memory_map,
            128,
            0x80000,
            0x81000,
            16,
        ))
        .unwrap();
    let fill_request = issue_line_fill_request(&mut contents, parent_id, line_start, request_end);
    contents.finish_issuing_parent(parent_id, &mut CacheMetrics::default());
    let expected_inflight_fills = contents.inflight_fills_snapshot();

    // Reject a response with the wrong address.
    let mismatched_fill_request = fill_request
        .cache_read_fragment(
            0x80001,
            128,
            fill_request.cache_fill_token().unwrap().clone(),
        )
        .unwrap();
    assert!(
        contents
            .complete_child_request(&mismatched_fill_request.to_response(&DummyMemory).unwrap())
            .is_err()
    );
    assert_eq!(contents.inflight_fills_snapshot(), expected_inflight_fills);

    // Issue a fill from a different cache.
    let mut other_cache_contents =
        CacheContents::new(engine.top(), create_single_line_cache_config());
    let (parent_id, line_start, request_end) = other_cache_contents
        .admit_request(create_read(
            engine.top(),
            &memory_map,
            128,
            0x80000,
            0x81000,
            16,
        ))
        .unwrap();
    let foreign_fill_request = issue_line_fill_request(
        &mut other_cache_contents,
        parent_id,
        line_start,
        request_end,
    );

    // Reject the foreign response without releasing the original slot.
    assert!(
        contents
            .complete_child_request(&foreign_fill_request.to_response(&DummyMemory).unwrap())
            .is_err()
    );
    assert_eq!(contents.inflight_fills_snapshot(), expected_inflight_fills);

    // Complete the original fill and record the released slot.
    let response = fill_request.to_response(&DummyMemory).unwrap();
    contents.complete_child_request(&response).unwrap();
    let empty_inflight_fills = contents.inflight_fills_snapshot();

    // Reject a duplicate response without changing the fill count.
    assert!(contents.complete_child_request(&response).is_err());
    assert_eq!(contents.inflight_fills_snapshot(), empty_inflight_fills);
}

#[test]
fn full_line_fetch_rejects_a_partial_line_at_the_address_space_boundary() {
    // Create a cache whose final line extends beyond the address space.
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let config = CacheConfig::new(3, 3, 1, 1, 0);
    config.validate().unwrap();
    let mut contents = CacheContents::<MemoryAccess>::new(engine.top(), config);

    // Admit a read of the final byte.
    let read_request = MemoryAccess::new(
        engine.top(),
        AccessType::ReadRequest,
        1,
        u64::MAX,
        0,
        DeviceId(0),
        DeviceId(1),
        0,
    );
    let (parent_id, line_start, request_end) = contents.admit_request(read_request).unwrap();

    // Reject the full-line fetch without reserving a fill slot.
    assert!(
        contents
            .process_parent_line(
                parent_id,
                line_start,
                request_end,
                &mut CacheMetrics::default()
            )
            .is_err()
    );
    assert_eq!(contents.inflight_fills_snapshot().current, 0);
    assert_eq!(contents.inflight_fills_snapshot().peak, 0);
}

#[test]
fn stale_barrier_response_cannot_complete_a_later_barrier() {
    // Complete and consume the first barrier response.
    let mut contents = create_coherent_cache_contents();
    let barrier = build_coherent_request(AccessType::BarrierRequest, 0, 0);
    let first = contents.begin_barrier(&barrier).unwrap().unwrap();
    contents.complete_barrier(first.to_ack().unwrap()).unwrap();
    contents.barrier_response.take();

    // Start a later barrier with a different child identity.
    let second = contents.begin_barrier(&barrier).unwrap().unwrap();

    // Reject the old response without supplying a new barrier response.
    assert!(contents.complete_barrier(first.to_ack().unwrap()).is_err());
    assert!(contents.barrier_response.is_none());

    // The rejected response must not consume the active barrier.
    contents.complete_barrier(second.to_ack().unwrap()).unwrap();
    assert!(contents.barrier_response.is_some());
}

#[test]
fn wrong_coherence_grant_leaves_the_exclusive_acquisition_pending() {
    let mut contents = create_coherent_cache_contents();
    let (parent_id, line_start, request_end) = contents
        .admit_request(build_coherent_request(
            AccessType::WriteNonPostedRequest,
            0,
            1,
        ))
        .unwrap();
    let acquisition = issue_line_fill_request(&mut contents, parent_id, line_start, request_end);
    assert!(
        contents
            .finish_issuing_parent(parent_id, &mut CacheMetrics::default())
            .is_none()
    );
    let expected_inflight_fills = contents.inflight_fills_snapshot();

    // A shared grant must neither release the slot nor satisfy the writer.
    let wrong = acquisition
        .to_ack()
        .unwrap()
        .with_coherence_op(Some(CoherenceOp::GrantShared));
    assert!(contents.complete_child_request(&wrong).is_err());
    assert_eq!(contents.inflight_fills_snapshot(), expected_inflight_fills);

    // The correct grant can still complete the same waiting parent.
    let ready = contents
        .complete_child_request(&wrong.with_coherence_op(Some(CoherenceOp::GrantExclusive)))
        .unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(contents.inflight_fills_snapshot().current, 0);
}

fn create_single_line_cache_config() -> CacheConfig {
    CacheConfig::new(128, 128, 1, 1, 0).with_admission_limits(4, 1)
}

fn issue_line_fill_request(
    contents: &mut CacheContents<MemoryAccess>,
    parent_id: ParentId,
    line_start: u128,
    request_end: u128,
) -> MemoryAccess {
    match contents
        .process_parent_line(
            parent_id,
            line_start,
            request_end,
            &mut CacheMetrics::default(),
        )
        .unwrap()
    {
        LineAction::Send(fill_request, _) => fill_request,
        _ => panic!("expected physical fill"),
    }
}

fn create_coherent_cache_contents() -> CacheContents<MemoryAccess> {
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let mut map = crate::memory::memory_map::MemoryMap::new();
    map.insert(0, MANAGER_MAP_SIZE_BYTES, DIRECTORY_DEVICE_ID)
        .unwrap();
    let mut contents = CacheContents::new(
        engine.top(),
        CacheConfig::new(
            COHERENT_LINE_SIZE_BYTES,
            COHERENT_BANDWIDTH_BYTES_PER_TICK,
            2,
            1,
            0,
        )
        .with_admission_limits(2, 1),
    );
    contents.coherence = Some(Rc::new(DirectoryCoherenceProtocol(CoherenceConfig::new(
        CACHE_DEVICE_ID,
        Rc::new(map),
    ))));
    contents
}

fn build_coherent_request(kind: AccessType, address: u64, bytes: usize) -> MemoryAccess {
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    MemoryAccess::new(
        engine.top(),
        kind,
        bytes,
        address,
        0,
        BACKING_MEMORY_DEVICE_ID,
        CLIENT_DEVICE_ID,
        REQUEST_HEADER_SIZE_BYTES,
    )
}

// Response construction in these timing-only tests does not read data.
struct DummyMemory;

impl ReadMemory for DummyMemory {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}
