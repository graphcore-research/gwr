// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_engine::engine::Engine;
use gwr_engine::traits::Routable;
use gwr_engine::types::{AccessType, DeviceId};

use super::*;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::traits::AccessMemory;
use crate::test_helpers::{create_default_memory_map, create_read};

#[test]
fn coalescing_and_late_invalidated_responses_retain_exact_slots() {
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let memory_map = Rc::new(create_default_memory_map());
    let read_request = || create_read(engine.top(), &memory_map, 128, 0x80000, 0x81000, 16);
    let mut contents = CacheContents::new(config());
    let (parent_id, line_start, request_end) = contents.admit_read_request(read_request()).unwrap();
    let fill_request = send(&mut contents, parent_id, line_start, request_end);
    assert!(
        contents
            .finish_issuing(parent_id, &mut CacheMetrics::default())
            .is_none()
    );
    let (coalesced_parent_id, line_start, request_end) =
        contents.admit_read_request(read_request()).unwrap();
    assert!(matches!(
        contents
            .process_line(
                coalesced_parent_id,
                line_start,
                request_end,
                &mut CacheMetrics::default()
            )
            .unwrap(),
        LineAction::Advance(_)
    ));
    assert!(
        contents
            .finish_issuing(coalesced_parent_id, &mut CacheMetrics::default())
            .is_none()
    );
    assert_eq!(contents.inflight_fills_snapshot().current, 1);
    contents.invalidate_range(0x80000, 1).unwrap();
    let (noallocate_parent_id, line_start, request_end) = contents
        .admit_read_request(read_request().with_cache_hint(CacheHintType::NoAllocate))
        .unwrap();
    assert!(matches!(
        contents
            .process_line(
                noallocate_parent_id,
                line_start,
                request_end,
                &mut CacheMetrics::default()
            )
            .unwrap(),
        LineAction::Blocked
    ));
    let response = fill_request.to_response(&contents).unwrap();
    assert_eq!(contents.response(&response).unwrap().len(), 2);
    assert_eq!(contents.inflight_fills_snapshot().current, 0);
    let replacement_fill_request =
        send(&mut contents, noallocate_parent_id, line_start, request_end);
    assert_eq!(replacement_fill_request.dst_addr(), 0x80000);
    assert_eq!(contents.inflight_fills_snapshot().current, 1);
    contents.finish_issuing(noallocate_parent_id, &mut CacheMetrics::default());
    contents
        .response(&replacement_fill_request.to_response(&contents).unwrap())
        .unwrap();
    assert_eq!(contents.inflight_fills_snapshot().current, 0);
    assert_eq!(contents.inflight_fills_snapshot().peak, 1);
}

#[test]
fn read_admission_rejects_other_access_types_without_changing_state() {
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let mut contents = CacheContents::new(config());
    let expected_inflight_fills = contents.inflight_fills_snapshot();

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
        assert!(
            contents.admit_read_request(request).is_err(),
            "{access_type:?}"
        );
        assert!(contents.parents.is_empty());
        assert_eq!(contents.next_parent, 0);
        assert_eq!(contents.inflight_fills_snapshot(), expected_inflight_fills);
    }
}

#[test]
fn bad_responses_leave_inflight_fills_unchanged() {
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let memory_map = Rc::new(create_default_memory_map());
    let mut contents = CacheContents::new(config());
    let (parent_id, line_start, request_end) = contents
        .admit_read_request(create_read(
            engine.top(),
            &memory_map,
            128,
            0x80000,
            0x81000,
            16,
        ))
        .unwrap();
    let fill_request = send(&mut contents, parent_id, line_start, request_end);
    contents.finish_issuing(parent_id, &mut CacheMetrics::default());
    let expected_inflight_fills = contents.inflight_fills_snapshot();
    let mismatched_fill_request = fill_request
        .cache_read_fragment(
            0x80001,
            128,
            fill_request.cache_fill_token().unwrap().clone(),
        )
        .unwrap();
    assert!(
        contents
            .response(&mismatched_fill_request.to_response(&contents).unwrap())
            .is_err()
    );
    assert_eq!(contents.inflight_fills_snapshot(), expected_inflight_fills);
    let mut other_cache_contents = CacheContents::new(config());
    let (parent_id, line_start, request_end) = other_cache_contents
        .admit_read_request(create_read(
            engine.top(),
            &memory_map,
            128,
            0x80000,
            0x81000,
            16,
        ))
        .unwrap();
    let foreign_fill_request = send(
        &mut other_cache_contents,
        parent_id,
        line_start,
        request_end,
    );
    assert!(
        contents
            .response(
                &foreign_fill_request
                    .to_response(&other_cache_contents)
                    .unwrap()
            )
            .is_err()
    );
    assert_eq!(contents.inflight_fills_snapshot(), expected_inflight_fills);
    let response = fill_request.to_response(&contents).unwrap();
    contents.response(&response).unwrap();
    let empty_inflight_fills = contents.inflight_fills_snapshot();
    assert!(contents.response(&response).is_err());
    assert_eq!(contents.inflight_fills_snapshot(), empty_inflight_fills);
}

#[test]
fn full_line_fetch_rejects_a_partial_line_at_the_address_space_boundary() {
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let config = CacheConfig::new(3, 3, 1, 1, 0);
    config.validate().unwrap();
    let mut contents = CacheContents::<MemoryAccess>::new(config);
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
    let (parent_id, line_start, request_end) = contents.admit_read_request(read_request).unwrap();
    assert!(
        contents
            .process_line(
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
fn basic_ways() {
    let line_size_bytes = 32;
    let num_sets = 1024;
    let num_ways = 4;
    let config = CacheConfig::new(line_size_bytes, 32, num_sets, num_ways, 8);
    let mut contents = CacheContents::<MemoryAccess>::new(config);
    let addresses: Vec<_> = (0..=num_ways)
        .map(|line_index| 0x0100_0000 + (line_index * line_size_bytes * num_sets * num_ways) as u64)
        .collect();

    for &address in addresses.iter().take(num_ways) {
        let (set_index, tag) = contents.location(address);
        assert!(contents.way(set_index, tag).is_none());
        let way_index = contents.reserve_line(set_index, tag).unwrap();
        assert_eq!(contents.way(set_index, tag), Some(way_index));
        assert!(matches!(
            contents.sets[set_index][way_index].validity,
            Validity::Missing
        ));
    }

    let (set_index, tag) = contents.location(addresses[num_ways]);
    contents.reserve_line(set_index, tag).unwrap();

    let (set_index, tag) = contents.location(addresses[0]);
    assert!(contents.way(set_index, tag).is_none());
    for &address in &addresses[1..] {
        let (set_index, tag) = contents.location(address);
        assert!(contents.way(set_index, tag).is_some());
    }
}

#[test]
fn invalidate() {
    let config = CacheConfig::new(32, 32, 1024, 4, 8);
    let mut contents = CacheContents::<MemoryAccess>::new(config);
    let address = 0x40000;
    let (set_index, tag) = contents.location(address);
    let way_index = contents.reserve_line(set_index, tag).unwrap();
    contents.sets[set_index][way_index].validity = Validity::Valid;

    contents.invalidate_range(address, 1).unwrap();

    assert!(matches!(
        contents.sets[set_index][way_index].validity,
        Validity::Missing
    ));
}

fn config() -> CacheConfig {
    CacheConfig::new(128, 128, 1, 1, 0).with_admission_limits(4, 1)
}

fn send(
    contents: &mut CacheContents<MemoryAccess>,
    parent_id: u64,
    line_start: u128,
    request_end: u128,
) -> MemoryAccess {
    match contents
        .process_line(
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
