// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_engine::traits::Routable;

use super::*;
use crate::memory::traits::{AccessMemory, CacheAccess, CacheFillToken, ReadMemory};

const CACHE_A_DEVICE_ID: DeviceId = DeviceId(1);
const CACHE_B_DEVICE_ID: DeviceId = DeviceId(2);
const DIRECTORY_DEVICE_ID: DeviceId = DeviceId(3);
const CACHE_C_DEVICE_ID: DeviceId = DeviceId(4);
const BACKING_MEMORY_DEVICE_ID: DeviceId = DeviceId(9);
const LINE_ADDRESS: u64 = 0;
const LINE_SIZE_BYTES: usize = 32;
const REQUEST_HEADER_SIZE_BYTES: usize = 16;
const BACKING_MEMORY_SIZE_BYTES: u64 = 4096;

#[test]
fn owner_reads_are_rejected_without_losing_exclusive_ownership() {
    for hint in [CacheHintType::Allocate, CacheHintType::NoAllocate] {
        let mut directory = create_directory();
        let acquisition = build_request(AccessType::WriteNonPostedRequest, CACHE_A_DEVICE_ID, 0);
        let grants = directory.handle_packet(acquisition.clone()).unwrap();
        assert_exclusive_grant(&grants, &acquisition);

        // An owner must read its local copy, which may be newer than memory.
        let read =
            build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 1).with_cache_hint(hint);
        assert!(directory.handle_packet(read).is_err());
        let entry = &directory.entries[&LINE_ADDRESS];
        assert_eq!(entry.ownership, Ownership::Owned(CACHE_A_DEVICE_ID));
        assert!(entry.active.is_none());

        // A subsequent reader must still invalidate the exclusive owner.
        let other_read = build_request(AccessType::ReadRequest, CACHE_B_DEVICE_ID, 2);
        let invalidations = directory.handle_packet(other_read.clone()).unwrap();
        assert_invalidation(&invalidations, &other_read, CACHE_A_DEVICE_ID);
    }
}

#[test]
fn stale_same_line_invalidate_ack_does_not_complete_new_acquisition() {
    // Give cache A ownership and verify its acquisition completes.
    let mut directory = create_directory();
    let initial_acquisition =
        build_request(AccessType::WriteNonPostedRequest, CACHE_A_DEVICE_ID, 0);
    let grants = directory
        .handle_packet(initial_acquisition.clone())
        .unwrap();
    assert_exclusive_grant(&grants, &initial_acquisition);

    // Transfer ownership to cache B after cache A acknowledges invalidation.
    let cache_b_acquisition =
        build_request(AccessType::WriteNonPostedRequest, CACHE_B_DEVICE_ID, 1);
    let invalidations = directory
        .handle_packet(cache_b_acquisition.clone())
        .unwrap();
    let initial_invalidation =
        assert_invalidation(&invalidations, &cache_b_acquisition, CACHE_A_DEVICE_ID);
    let previous_token = initial_invalidation.coherence_token().unwrap().clone();
    let grants = directory
        .handle_packet(build_invalidation_ack(initial_invalidation.clone()))
        .unwrap();
    assert_exclusive_grant(&grants, &cache_b_acquisition);

    // Reacquiring ownership for cache A must invalidate cache B with a new
    // token.
    let cache_a_reacquisition =
        build_request(AccessType::WriteNonPostedRequest, CACHE_A_DEVICE_ID, 2);
    let invalidations = directory
        .handle_packet(cache_a_reacquisition.clone())
        .unwrap();
    let current_invalidation =
        assert_invalidation(&invalidations, &cache_a_reacquisition, CACHE_B_DEVICE_ID);
    let current_token = current_invalidation.coherence_token().unwrap();
    assert!(!previous_token.matches(current_token));
    let current_acknowledgement = build_invalidation_ack(current_invalidation.clone());

    // Change only the token, so rejection specifically exercises the stale
    // round.
    let stale_acknowledgement = current_acknowledgement
        .clone()
        .with_coherence_token(Some(previous_token));
    assert!(directory.handle_packet(stale_acknowledgement).is_err());

    // Rejection must preserve cache B's ownership and cache A's pending
    // acquisition.
    let entry = &directory.entries[&LINE_ADDRESS];
    assert_eq!(entry.ownership, Ownership::Owned(CACHE_B_DEVICE_ID));
    assert!(entry.queued.is_empty());
    let active_acquisition = entry
        .active
        .as_ref()
        .expect("cache A must still be waiting");
    assert_same_fill_identity(&active_acquisition.request, &cache_a_reacquisition);
    let Phase::Invalidations {
        pending_sender_device_ids,
        token,
        ..
    } = &active_acquisition.phase
    else {
        panic!("the acquisition must still be waiting for invalidation");
    };
    assert_eq!(
        pending_sender_device_ids,
        &BTreeSet::from([CACHE_B_DEVICE_ID])
    );
    assert!(token.matches(current_token));

    // Only the current acknowledgement completes cache A's acquisition.
    let grants = directory.handle_packet(current_acknowledgement).unwrap();
    assert_exclusive_grant(&grants, &cache_a_reacquisition);
    let entry = &directory.entries[&LINE_ADDRESS];
    assert_eq!(entry.ownership, Ownership::Owned(CACHE_A_DEVICE_ID));
    assert!(entry.active.is_none());
    assert!(entry.queued.is_empty());
}

#[test]
fn foreign_memory_response_preserves_pending_work() {
    // Leave a shared read waiting for its backing-memory response.
    let mut directory = create_directory();
    let memory_read = directory
        .handle_packet(build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 0))
        .unwrap()
        .remove(0);

    // Reject a response with a foreign fill identity.
    let foreign = build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 0)
        .with_routing(BACKING_MEMORY_DEVICE_ID, DIRECTORY_DEVICE_ID)
        .to_response(&TestMemory)
        .unwrap();
    assert!(directory.handle_packet(foreign).is_err());

    // Accept the original response once, preserving the pending read.
    let response = memory_read.to_response(&TestMemory).unwrap();
    assert_eq!(
        directory.handle_packet(response.clone()).unwrap()[0].coherence_op(),
        Some(CoherenceOp::GrantShared)
    );
    assert!(directory.handle_packet(response).is_err());
}

#[test]
fn noallocate_read_resolves_dirty_owner_without_registering_sharer() {
    // Give the first cache dirty ownership of the line.
    let mut directory = create_directory();
    directory
        .handle_packet(build_request(
            AccessType::WriteNonPostedRequest,
            CACHE_A_DEVICE_ID,
            0,
        ))
        .unwrap();

    // A nonallocating reader must resolve that dirty owner first.
    let read = build_request(AccessType::ReadRequest, CACHE_B_DEVICE_ID, 1)
        .with_cache_hint(crate::memory::CacheHintType::NoAllocate);
    let invalidation = directory.handle_packet(read).unwrap().remove(0);
    let writeback = build_request(AccessType::WriteRequest, CACHE_A_DEVICE_ID, 3)
        .with_coherence_op(Some(CoherenceOp::Writeback));
    let memory_write = directory.handle_packet(writeback).unwrap().remove(0);
    assert_eq!(memory_write.dst_device(), BACKING_MEMORY_DEVICE_ID);
    assert!(
        directory
            .handle_packet(memory_write.to_ack().unwrap())
            .unwrap()
            .is_empty()
    );

    // Acknowledge invalidation and complete the backing-memory read.
    let ack = invalidation
        .with_coherence_op(Some(CoherenceOp::InvalidateAck))
        .with_routing(DIRECTORY_DEVICE_ID, CACHE_A_DEVICE_ID);
    let memory_read = directory.handle_packet(ack).unwrap().remove(0);
    directory
        .handle_packet(memory_read.to_response(&TestMemory).unwrap())
        .unwrap();

    // Reacquiring ownership must not invalidate the nonallocating reader.
    let grant = directory
        .handle_packet(build_request(
            AccessType::WriteNonPostedRequest,
            CACHE_A_DEVICE_ID,
            4,
        ))
        .unwrap();
    assert_eq!(grant.len(), 1);
    assert_eq!(grant[0].coherence_op(), Some(CoherenceOp::GrantExclusive));
}

#[test]
fn dirty_eviction_holds_barrier_until_memory_acknowledges_writeback() {
    let mut directory = create_directory();
    directory
        .handle_packet(build_request(AccessType::Control, CACHE_A_DEVICE_ID, 0))
        .unwrap();
    let writeback = build_request(AccessType::WriteRequest, CACHE_A_DEVICE_ID, 1)
        .with_coherence_op(Some(CoherenceOp::Writeback));
    let memory_write = directory
        .handle_packet(writeback.clone())
        .unwrap()
        .remove(0);
    let barrier =
        build_request(AccessType::BarrierRequest, CACHE_A_DEVICE_ID, 2).with_coherence_op(None);
    assert!(directory.handle_packet(barrier.clone()).unwrap().is_empty());
    assert_eq!(
        memory_write.access_type(),
        AccessType::WriteNonPostedRequest
    );
    assert_eq!(memory_write.dst_device(), BACKING_MEMORY_DEVICE_ID);
    assert_same_fill_identity(&memory_write, &writeback);

    // Later requests cannot pass the barrier while the write is in transit.
    let acquisition = build_request(AccessType::WriteNonPostedRequest, CACHE_B_DEVICE_ID, 3);
    assert!(
        directory
            .handle_packet(acquisition.clone())
            .unwrap()
            .is_empty()
    );
    let acknowledgement = memory_write.to_ack().unwrap();
    let foreign_acknowledgement =
        build_request(AccessType::WriteNonPostedRequest, CACHE_A_DEVICE_ID, 4)
            .with_routing(BACKING_MEMORY_DEVICE_ID, DIRECTORY_DEVICE_ID)
            .to_ack()
            .unwrap();
    assert!(directory.handle_packet(foreign_acknowledgement).is_err());

    // The write acknowledgement is consumed, then the barrier releases the
    // writer.
    let output = directory.handle_packet(acknowledgement.clone()).unwrap();
    assert_eq!(output.len(), 2);
    assert_eq!(output[0].access_type(), AccessType::BarrierResponse);
    assert_same_fill_identity(&output[0], &barrier);
    assert_exclusive_grant(&output[1..], &acquisition);
    assert!(directory.handle_packet(acknowledgement).is_err());
}

#[test]
fn invalidation_waits_for_both_writeback_and_invalidation_acknowledgements() {
    for memory_ack_first in [false, true] {
        let mut directory = create_directory();
        directory
            .handle_packet(build_request(AccessType::Control, CACHE_A_DEVICE_ID, 0))
            .unwrap();
        let acquisition = build_request(AccessType::WriteNonPostedRequest, CACHE_B_DEVICE_ID, 1);
        let invalidate = directory
            .handle_packet(acquisition.clone())
            .unwrap()
            .remove(0);
        let writeback = build_request(AccessType::WriteRequest, CACHE_A_DEVICE_ID, 2)
            .with_coherence_op(Some(CoherenceOp::Writeback));
        let memory_write = directory.handle_packet(writeback).unwrap().remove(0);
        assert_eq!(
            memory_write.access_type(),
            AccessType::WriteNonPostedRequest
        );
        let memory_acknowledgement = memory_write.to_ack().unwrap();
        let invalidation_acknowledgement = build_invalidation_ack(invalidate);
        let acknowledgements = if memory_ack_first {
            [memory_acknowledgement, invalidation_acknowledgement]
        } else {
            [invalidation_acknowledgement, memory_acknowledgement]
        };
        let [first_acknowledgement, second_acknowledgement] = acknowledgements;
        assert!(
            directory
                .handle_packet(first_acknowledgement)
                .unwrap()
                .is_empty()
        );
        let grants = directory.handle_packet(second_acknowledgement).unwrap();
        assert_exclusive_grant(&grants, &acquisition);
    }
}

#[test]
fn barrier_holds_later_requests_until_memory_completes() {
    // Leave the first read outstanding at backing memory.
    let mut directory = create_directory();
    let memory_read = directory
        .handle_packet(build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 0))
        .unwrap()
        .remove(0);

    // Queue a barrier and a later writer behind the pending read.
    let barrier = build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 1)
        .with_access_type(AccessType::BarrierRequest);
    assert!(directory.handle_packet(barrier).unwrap().is_empty());
    assert!(
        directory
            .handle_packet(build_request(
                AccessType::WriteNonPostedRequest,
                CACHE_B_DEVICE_ID,
                2
            ))
            .unwrap()
            .is_empty()
    );

    // Complete the read, then release the barrier before the writer.
    let ready = directory
        .handle_packet(memory_read.to_response(&TestMemory).unwrap())
        .unwrap();
    assert_eq!(ready[0].coherence_op(), Some(CoherenceOp::GrantShared));
    assert_eq!(ready[1].access_type(), AccessType::BarrierResponse);
    assert_eq!(ready[2].coherence_op(), Some(CoherenceOp::Invalidate));
}

#[test]
fn barriers_wait_for_all_lines_and_replay_in_order_after_queued_grants() {
    let mut directory = create_directory();
    let first_read = build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 0);
    let second_read = first_read
        .build_coherence_child_request(
            AccessType::ReadRequest,
            LINE_SIZE_BYTES as u64,
            LINE_SIZE_BYTES,
            CacheFillToken {
                owner: Rc::new(()),
                ordinal: 1,
            },
        )
        .unwrap();
    let first_memory_access = directory.handle_packet(first_read).unwrap().remove(0);
    let second_memory_access = directory.handle_packet(second_read).unwrap().remove(0);

    // Both upgrades can complete synchronously once the first read returns.
    for ordinal in [2, 3] {
        assert!(
            directory
                .handle_packet(build_request(
                    AccessType::Control,
                    CACHE_A_DEVICE_ID,
                    ordinal
                ))
                .unwrap()
                .is_empty()
        );
    }
    let first_barrier =
        build_request(AccessType::BarrierRequest, CACHE_A_DEVICE_ID, 4).with_coherence_op(None);
    assert!(
        directory
            .handle_packet(first_barrier.clone())
            .unwrap()
            .is_empty()
    );
    let second_barrier =
        build_request(AccessType::BarrierRequest, CACHE_A_DEVICE_ID, 5).with_coherence_op(None);
    assert!(
        directory
            .handle_packet(second_barrier.clone())
            .unwrap()
            .is_empty()
    );
    assert!(
        directory
            .handle_packet(build_request(AccessType::Control, CACHE_B_DEVICE_ID, 6))
            .unwrap()
            .is_empty()
    );

    let first_output = directory
        .handle_packet(first_memory_access.to_response(&TestMemory).unwrap())
        .unwrap();
    assert_eq!(
        first_output
            .iter()
            .map(|packet| packet.coherence_op())
            .collect::<Vec<_>>(),
        vec![
            Some(CoherenceOp::GrantShared),
            Some(CoherenceOp::GrantExclusive),
            Some(CoherenceOp::GrantExclusive)
        ],
    );

    // The other line still holds both barriers. Its response releases them
    // before the later writer can invalidate the owner of the first line.
    let second_output = directory
        .handle_packet(second_memory_access.to_response(&TestMemory).unwrap())
        .unwrap();
    assert_eq!(second_output.len(), 4);
    assert_eq!(
        second_output[0].coherence_op(),
        Some(CoherenceOp::GrantShared)
    );
    for (response, request) in second_output[1..3]
        .iter()
        .zip([first_barrier, second_barrier])
    {
        assert_eq!(response.access_type(), AccessType::BarrierResponse);
        assert_same_fill_identity(response, &request);
    }
    assert_eq!(
        second_output[3].coherence_op(),
        Some(CoherenceOp::Invalidate)
    );
    let grants = directory
        .handle_packet(build_invalidation_ack(second_output[3].clone()))
        .unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].coherence_op(), Some(CoherenceOp::GrantExclusive));
}

#[test]
fn manager_rejects_zero_and_misaligned_line_sizes() {
    // Reject a zero line size.
    let mut config = create_directory().config;
    config.line_size_bytes = 0;
    assert!(config.validate().is_err());

    // Reject a line size that does not align the backing range.
    config.line_size_bytes = 48;
    assert!(config.validate().is_err());

    // Keep an aligned configuration valid.
    config.line_size_bytes = LINE_SIZE_BYTES;
    assert!(config.validate().is_ok());
}

#[test]
fn exclusive_grant_waits_for_every_sharer_and_rejects_duplicate_ack() {
    // Populate two shared copies of the line.
    let mut directory = create_directory();
    for (source_device_id, fill_ordinal) in [(CACHE_A_DEVICE_ID, 1), (CACHE_B_DEVICE_ID, 2)] {
        let read = directory
            .handle_packet(build_request(
                AccessType::ReadRequest,
                source_device_id,
                fill_ordinal,
            ))
            .unwrap()
            .remove(0);
        directory
            .handle_packet(read.to_response(&TestMemory).unwrap())
            .unwrap();
    }

    assert_eq!(
        directory.entries[&LINE_ADDRESS].ownership,
        Ownership::Shared(BTreeSet::from([CACHE_A_DEVICE_ID, CACHE_B_DEVICE_ID]))
    );

    // Request exclusive ownership and invalidate both sharers.
    let mut invalidations = directory
        .handle_packet(build_request(
            AccessType::WriteNonPostedRequest,
            CACHE_C_DEVICE_ID,
            4,
        ))
        .unwrap();
    assert_eq!(invalidations.len(), 2);

    // One acknowledgement is insufficient; a duplicate is invalid.
    let first = build_invalidation_ack(invalidations.remove(0));
    assert!(directory.handle_packet(first.clone()).unwrap().is_empty());
    assert!(directory.handle_packet(first).is_err());

    assert_eq!(
        directory.entries[&LINE_ADDRESS].ownership,
        Ownership::Shared(BTreeSet::from([CACHE_B_DEVICE_ID]))
    );

    // The remaining sharer releases the exclusive grant.
    let grant = directory
        .handle_packet(build_invalidation_ack(invalidations.remove(0)))
        .unwrap();
    assert_eq!(grant[0].coherence_op(), Some(CoherenceOp::GrantExclusive));
    assert_eq!(
        directory.entries[&LINE_ADDRESS].ownership,
        Ownership::Owned(CACHE_C_DEVICE_ID)
    );
}

#[test]
fn same_line_reads_replay_after_the_original_memory_response() {
    // Queue a second reader behind the first backing-memory read.
    let mut directory = create_directory();
    let first = directory
        .handle_packet(build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 1))
        .unwrap()
        .remove(0);
    assert!(
        directory
            .handle_packet(build_request(AccessType::ReadRequest, CACHE_B_DEVICE_ID, 2))
            .unwrap()
            .is_empty()
    );

    // Completing the first read starts the queued read.
    let completed = directory
        .handle_packet(first.to_response(&TestMemory).unwrap())
        .unwrap();
    assert_eq!(completed.len(), 2);
    assert_eq!(completed[0].coherence_op(), Some(CoherenceOp::GrantShared));

    // Reject a replay of the old response before accepting the new one.
    let second = completed[1].clone();
    assert!(
        directory
            .handle_packet(first.to_response(&TestMemory).unwrap())
            .is_err()
    );
    assert_eq!(
        directory
            .handle_packet(second.to_response(&TestMemory).unwrap())
            .unwrap()[0]
            .dst_device(),
        CACHE_B_DEVICE_ID
    );
}

#[test]
fn writeback_and_ack_bypass_barrier_to_complete_prior_acquisition() {
    // Start a read that must invalidate the current dirty owner.
    let mut directory = create_directory();
    directory
        .handle_packet(build_request(
            AccessType::WriteNonPostedRequest,
            CACHE_A_DEVICE_ID,
            1,
        ))
        .unwrap();
    let invalidate = directory
        .handle_packet(build_request(AccessType::ReadRequest, CACHE_B_DEVICE_ID, 2))
        .unwrap()
        .remove(0);

    // Queue a barrier while the ownership transfer is pending.
    let barrier = build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 3)
        .with_access_type(AccessType::BarrierRequest);
    assert!(directory.handle_packet(barrier).unwrap().is_empty());

    // Allow the owner writeback and acknowledgement through the barrier.
    let wb = build_request(AccessType::WriteRequest, CACHE_A_DEVICE_ID, 4)
        .with_coherence_op(Some(CoherenceOp::Writeback));
    let memory_write = directory.handle_packet(wb).unwrap().remove(0);
    assert_eq!(
        memory_write.access_type(),
        AccessType::WriteNonPostedRequest
    );
    assert!(
        directory
            .handle_packet(build_invalidation_ack(invalidate))
            .unwrap()
            .is_empty()
    );
    let memory = directory
        .handle_packet(memory_write.to_ack().unwrap())
        .unwrap()
        .remove(0);

    // Complete the read before releasing the barrier.
    let output = directory
        .handle_packet(memory.to_response(&TestMemory).unwrap())
        .unwrap();
    assert_eq!(output[0].coherence_op(), Some(CoherenceOp::GrantShared));
    assert_eq!(output[1].access_type(), AccessType::BarrierResponse);
}

#[test]
fn mismatched_memory_response_ranges_leave_active_request_intact() {
    // Start a backing-memory read for one whole line.
    let mut directory = create_directory();
    let read = directory
        .handle_packet(build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 1))
        .unwrap()
        .remove(0);

    // Reject a response with the right identity but the wrong range.
    let wrong = read
        .build_coherence_child_request(
            AccessType::ReadRequest,
            1,
            LINE_SIZE_BYTES - 1,
            read.cache_fill_token().unwrap().clone(),
        )
        .unwrap();
    assert!(
        directory
            .handle_packet(wrong.to_response(&TestMemory).unwrap())
            .is_err()
    );

    // The original read must still complete successfully.
    assert_eq!(
        directory
            .handle_packet(read.to_response(&TestMemory).unwrap())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn nonallocating_write_forwards_to_memory_without_retaining_an_owner() {
    // Forward a nonallocating write directly to backing memory.
    let mut directory = create_directory();
    let write = build_request(AccessType::WriteNonPostedRequest, CACHE_A_DEVICE_ID, 0)
        .with_cache_hint(CacheHintType::NoAllocate);
    let forwarded = directory.handle_packet(write).unwrap().remove(0);
    assert_eq!(forwarded.dst_device(), BACKING_MEMORY_DEVICE_ID);
    assert_eq!(forwarded.coherence_op(), None);

    // Return its response without retaining cache ownership or sharers.
    let response = directory
        .handle_packet(forwarded.to_ack().unwrap())
        .unwrap();
    assert_eq!(response[0].coherence_op(), None);
    assert_eq!(
        directory.entries[&LINE_ADDRESS].ownership,
        Ownership::Uncached
    );
}

#[test]
fn posted_write_waits_for_memory_without_replying_to_requester() {
    // Track the posted write through an acknowledged memory operation.
    let mut directory = create_directory();
    let write = build_request(AccessType::WriteRequest, CACHE_A_DEVICE_ID, 0)
        .with_cache_hint(CacheHintType::NoAllocate);
    let forwarded = directory.handle_packet(write.clone()).unwrap();
    assert_eq!(forwarded.len(), 1);
    let memory_write = &forwarded[0];
    assert_eq!(
        memory_write.access_type(),
        AccessType::WriteNonPostedRequest
    );
    assert_eq!(memory_write.dst_device(), BACKING_MEMORY_DEVICE_ID);
    assert!(fill_tokens_match(
        memory_write.cache_fill_token(),
        write.cache_fill_token()
    ));

    // Neither a same-line read nor a barrier may pass the pending write.
    assert!(
        directory
            .handle_packet(build_request(AccessType::ReadRequest, CACHE_B_DEVICE_ID, 1))
            .unwrap()
            .is_empty()
    );
    let barrier =
        build_request(AccessType::BarrierRequest, CACHE_A_DEVICE_ID, 2).with_coherence_op(None);
    assert!(directory.handle_packet(barrier).unwrap().is_empty());

    // Consuming the write acknowledgement emits only the queued memory read,
    // with no write response to the original requester.
    let after_write = directory
        .handle_packet(memory_write.to_ack().unwrap())
        .unwrap();
    assert_eq!(after_write.len(), 1);
    let memory_read = &after_write[0];
    assert_eq!(memory_read.access_type(), AccessType::ReadRequest);
    assert_eq!(memory_read.dst_device(), BACKING_MEMORY_DEVICE_ID);

    // Complete the queued read before releasing the barrier.
    let after_read = directory
        .handle_packet(memory_read.to_response(&TestMemory).unwrap())
        .unwrap();
    assert_eq!(after_read.len(), 2);
    assert_eq!(after_read[0].coherence_op(), Some(CoherenceOp::GrantShared));
    assert_eq!(after_read[0].dst_device(), CACHE_B_DEVICE_ID);
    assert_eq!(after_read[1].access_type(), AccessType::BarrierResponse);
    assert_eq!(after_read[1].dst_device(), CACHE_A_DEVICE_ID);
}

#[test]
fn cross_line_and_unmapped_requests_fail_before_directory_mutation() {
    // Start with an empty directory and a valid request identity.
    let mut directory = create_directory();
    let read = build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 1);

    // Reject invalid ranges without allocating directory state.
    for (address, bytes) in [
        (LINE_ADDRESS, 2 * LINE_SIZE_BYTES),
        (BACKING_MEMORY_SIZE_BYTES, LINE_SIZE_BYTES),
        (u64::MAX - 31, LINE_SIZE_BYTES),
    ] {
        let packet = read
            .build_coherence_child_request(
                AccessType::ReadRequest,
                address,
                bytes,
                read.cache_fill_token().unwrap().clone(),
            )
            .unwrap();
        assert!(directory.handle_packet(packet).is_err());
        assert!(directory.entries.is_empty());
    }
}

#[test]
fn queued_noallocate_read_does_not_unregister_an_existing_resident_sharer() {
    // Queue a nonallocating read behind an allocating read by the same cache.
    let mut directory = create_directory();
    let first = directory
        .handle_packet(build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 1))
        .unwrap()
        .remove(0);
    let noallocate = build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 2)
        .with_cache_hint(CacheHintType::NoAllocate);
    assert!(directory.handle_packet(noallocate).unwrap().is_empty());

    // Complete both reads while preserving the resident shared copy.
    let output = directory
        .handle_packet(first.to_response(&TestMemory).unwrap())
        .unwrap();
    assert_eq!(output.len(), 2);
    directory
        .handle_packet(output[1].to_response(&TestMemory).unwrap())
        .unwrap();

    // A later writer must still invalidate the resident sharer.
    let write = directory
        .handle_packet(build_request(
            AccessType::WriteNonPostedRequest,
            CACHE_B_DEVICE_ID,
            3,
        ))
        .unwrap();
    assert_eq!(write[0].coherence_op(), Some(CoherenceOp::Invalidate));
    assert_eq!(write[0].dst_device(), CACHE_A_DEVICE_ID);
}

#[test]
fn mapped_final_address_line_does_not_overflow_backing_lookup() {
    // Map the final two lines of the address space.
    let mut map = MemoryMap::new();
    map.insert(
        u64::MAX - 63,
        2 * LINE_SIZE_BYTES as u64,
        BACKING_MEMORY_DEVICE_ID,
    )
    .unwrap();
    let mut directory = Directory::new(CoherencyManagerConfig::new(
        LINE_SIZE_BYTES,
        DIRECTORY_DEVICE_ID,
        map,
    ));
    let read = build_request(AccessType::ReadRequest, CACHE_A_DEVICE_ID, 1);

    // Construct a whole-line read ending at the final address.
    let packet = read
        .build_coherence_child_request(
            AccessType::ReadRequest,
            u64::MAX - 31,
            LINE_SIZE_BYTES,
            read.cache_fill_token().unwrap().clone(),
        )
        .unwrap();

    // Forward and complete the read without overflowing the range.
    let forwarded = directory.handle_packet(packet).unwrap().remove(0);
    assert_eq!(forwarded.dst_addr(), u64::MAX - 31);
    assert_eq!(
        directory
            .handle_packet(forwarded.to_response(&TestMemory).unwrap())
            .unwrap()
            .len(),
        1
    );
}

fn create_directory() -> Directory<MemoryAccess> {
    let mut map = MemoryMap::new();
    map.insert(
        LINE_ADDRESS,
        BACKING_MEMORY_SIZE_BYTES,
        BACKING_MEMORY_DEVICE_ID,
    )
    .unwrap();
    Directory::new(CoherencyManagerConfig::new(
        LINE_SIZE_BYTES,
        DIRECTORY_DEVICE_ID,
        map,
    ))
}

fn build_request(
    access_type: AccessType,
    source_device_id: DeviceId,
    fill_ordinal: u64,
) -> MemoryAccess {
    let engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let packet = MemoryAccess::new(
        engine.top(),
        access_type,
        LINE_SIZE_BYTES,
        LINE_ADDRESS,
        0,
        DIRECTORY_DEVICE_ID,
        source_device_id,
        REQUEST_HEADER_SIZE_BYTES,
    );
    packet
        .build_coherence_child_request(
            access_type,
            LINE_ADDRESS,
            LINE_SIZE_BYTES,
            CacheFillToken {
                owner: Rc::new(()),
                ordinal: fill_ordinal,
            },
        )
        .unwrap()
        .with_coherence_op(Some(if access_type == AccessType::ReadRequest {
            CoherenceOp::SharedRead
        } else {
            CoherenceOp::ExclusiveWrite
        }))
}

fn build_invalidation_ack(packet: MemoryAccess) -> MemoryAccess {
    let sender_device_id = packet.dst_device();
    packet
        .with_coherence_op(Some(CoherenceOp::InvalidateAck))
        .with_routing(DIRECTORY_DEVICE_ID, sender_device_id)
}

fn assert_exclusive_grant(packets: &[MemoryAccess], acquisition: &MemoryAccess) {
    let [grant] = packets else {
        panic!("expected exactly one exclusive grant");
    };
    assert_eq!(grant.access_type(), AccessType::WriteNonPostedResponse);
    assert_eq!(grant.coherence_op(), Some(CoherenceOp::GrantExclusive));
    assert_eq!(grant.src_device(), DIRECTORY_DEVICE_ID);
    assert_eq!(grant.dst_device(), acquisition.src_device());
    assert_eq!(grant.dst_addr(), LINE_ADDRESS);
    assert_eq!(grant.access_size_bytes(), LINE_SIZE_BYTES);
    assert_same_fill_identity(grant, acquisition);
}

fn assert_invalidation<'a>(
    packets: &'a [MemoryAccess],
    acquisition: &MemoryAccess,
    owner_device_id: DeviceId,
) -> &'a MemoryAccess {
    let [invalidation] = packets else {
        panic!("expected exactly one invalidation");
    };
    assert_eq!(invalidation.access_type(), AccessType::Control);
    assert_eq!(invalidation.coherence_op(), Some(CoherenceOp::Invalidate));
    assert_eq!(invalidation.src_device(), DIRECTORY_DEVICE_ID);
    assert_eq!(invalidation.dst_device(), owner_device_id);
    assert_eq!(invalidation.dst_addr(), LINE_ADDRESS);
    assert_eq!(invalidation.access_size_bytes(), LINE_SIZE_BYTES);
    assert_same_fill_identity(invalidation, acquisition);
    invalidation
}

fn assert_same_fill_identity(packet: &MemoryAccess, acquisition: &MemoryAccess) {
    let actual_fill_token = packet.cache_fill_token().unwrap();
    let expected_fill_token = acquisition.cache_fill_token().unwrap();
    assert!(Rc::ptr_eq(
        &actual_fill_token.owner,
        &expected_fill_token.owner
    ));
    assert_eq!(actual_fill_token.ordinal, expected_fill_token.ordinal);
}

struct TestMemory;

impl ReadMemory for TestMemory {
    fn read(&self) -> Vec<u8> {
        vec![0; LINE_SIZE_BYTES]
    }
}

// Unit tests drive the unclocked directory to quiescence. Port tests exercise
// the real worker and its shared clocked processing budget.
impl Directory<MemoryAccess> {
    fn handle_packet(&mut self, packet: MemoryAccess) -> Result<Vec<MemoryAccess>, SimError> {
        let mut output = self.process_packet(packet)?;
        while let Some(messages) = self.process_pending()? {
            output.extend(messages);
        }
        Ok(output)
    }
}
