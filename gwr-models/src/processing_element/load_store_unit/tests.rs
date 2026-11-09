// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::collections::BTreeMap;
use std::iter::once;

use futures::{poll, try_join};

use super::*;
use crate::memory::traits::AccessMemory;
use crate::processing_element::compute::ComputeConfig;

const LSU_ACCESS_BYTES: usize = 64;
const MEMORY_CAPACITY_BYTES: u64 = 256;

fn load_store_unit_fixture(engine: &mut Engine) -> Rc<LoadStoreUnit> {
    let mut map = MemoryMap::new();
    map.insert(0, MEMORY_CAPACITY_BYTES, DeviceId(0)).unwrap();
    let entity = Rc::new(Entity::new(engine.top(), "lsu"));
    Rc::new(LoadStoreUnit {
        entity: entity.clone(),
        spawner: engine.spawner(),
        clock: engine.default_clock(),
        max_access_size_bytes: LSU_ACCESS_BYTES,
        rx: RefCell::new(None),
        tx: RefCell::new(None),
        state: Rc::new(LsuState {
            entity,
            memory_map: Rc::new(map),
            device_id: DeviceId(1),
            overhead_size_bytes: 0,
            active_request_slots: RefCell::new(vec![ActiveRequestSlot::default()]),
            pending_request_indices: RefCell::new(VecDeque::new()),
            new_request: Repeated::new(()),
            slot_available: Repeated::new(()),
            request_arbiter: RequestArbiter::default(),
        }),
    })
}

async fn wait_for_issued_request(lsu: &LoadStoreUnit) -> Result<MemoryAccess, SimError> {
    loop {
        if let Some(idx) = lsu.state.pending_request_indices.borrow_mut().pop_front() {
            return Ok(lsu.state.active_request_slots.borrow_mut()[idx]
                .request
                .take()
                .unwrap());
        }
        lsu.clock.wait_ticks(1)?.await;
    }
}

fn deliver_memory_response(lsu: &LoadStoreUnit, request: &MemoryAccess) -> SimResult {
    lsu.state
        .put_response_in_active_request_slot(MemoryAccess::new(
            &lsu.entity,
            match request.access_type() {
                AccessType::ReadRequest => AccessType::ReadResponse,
                AccessType::WriteNonPostedRequest => AccessType::WriteNonPostedResponse,
                other => panic!("unexpected request type {other:?}"),
            },
            request.access_size_bytes(),
            request.dst_addr(),
            request.src_addr(),
            request.src_device(),
            request.dst_device(),
            0,
        ))
}

fn register_lsu_with_memory_region(
    engine: &mut Engine,
    start: u64,
    bytes: u64,
    access_bytes: usize,
) -> Result<Rc<LoadStoreUnit>, SimError> {
    let mut map = MemoryMap::new();
    map.insert(start, bytes, DeviceId(0))?;
    let config = ProcessingElementConfig {
        num_active_requests: 1,
        lsu_access_bytes: access_bytes,
        overhead_size_bytes: 0,
        sram_bytes: MEMORY_CAPACITY_BYTES as usize,
        compute: ComputeConfig {
            execution_units: BTreeMap::new(),
        },
        operation_classes: BTreeMap::new(),
    };
    let clock = engine.default_clock();
    LoadStoreUnit::new_and_register(
        engine,
        &clock,
        engine.top(),
        None,
        &config,
        &Rc::new(map),
        DeviceId(1),
    )
}

#[test]
fn rejects_a_memory_region_start_inside_an_lsu_block() {
    const REGION_START: u64 = LSU_ACCESS_BYTES as u64 / 2;
    // The exclusive end is aligned, isolating the invalid start boundary.
    const REGION_BYTES: u64 = MEMORY_CAPACITY_BYTES - REGION_START;
    let mut engine = Engine::default();

    let error =
        register_lsu_with_memory_region(&mut engine, REGION_START, REGION_BYTES, LSU_ACCESS_BYTES)
            .unwrap_err();

    assert!(error.to_string().contains("LSU access size"), "{error}");
}

#[test]
fn rejects_a_memory_region_end_inside_an_lsu_block() {
    const REGION_BYTES: u64 = MEMORY_CAPACITY_BYTES - LSU_ACCESS_BYTES as u64 / 2;
    let mut engine = Engine::default();

    let error = register_lsu_with_memory_region(&mut engine, 0, REGION_BYTES, LSU_ACCESS_BYTES)
        .unwrap_err();

    assert!(error.to_string().contains("LSU access size"), "{error}");
}

#[test]
fn accepts_a_memory_region_aligned_to_a_non_power_of_two_lsu_size() {
    const ACCESS_BYTES: usize = 3;
    const REGION_START: u64 = ACCESS_BYTES as u64;
    const REGION_BYTES: u64 = 2 * ACCESS_BYTES as u64;
    let mut engine = Engine::default();

    register_lsu_with_memory_region(&mut engine, REGION_START, REGION_BYTES, ACCESS_BYTES).unwrap();
}

#[test]
fn accepts_an_aligned_memory_region_ending_at_the_last_address() {
    const REGION_BYTES: u64 = LSU_ACCESS_BYTES as u64;
    const REGION_START: u64 = u64::MAX - (REGION_BYTES - 1);
    let mut engine = Engine::default();

    register_lsu_with_memory_region(&mut engine, REGION_START, REGION_BYTES, LSU_ACCESS_BYTES)
        .unwrap();
}

#[test]
fn waiting_read_issues_between_store_requests() {
    let mut engine = Engine::default();
    let lsu = load_store_unit_fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "fair"));
        let store = lsu.access_ranges(
            AccessType::WriteNonPostedRequest,
            once(0..2 * LSU_ACCESS_BYTES as u128),
            &lanes,
            "store",
            &group,
        );
        let load = lsu.access_ranges(
            AccessType::ReadRequest,
            once(2 * LSU_ACCESS_BYTES as u128..3 * LSU_ACCESS_BYTES as u128),
            &lanes,
            "load",
            &group,
        );
        let driver = async {
            // Both transfers contend for the single slot. The two-request
            // store must give the waiting read a turn between its requests.
            for expected in [
                AccessType::WriteNonPostedRequest,
                AccessType::ReadRequest,
                AccessType::WriteNonPostedRequest,
            ] {
                let request = wait_for_issued_request(&lsu).await?;
                assert_eq!(request.access_type(), expected);
                deliver_memory_response(&lsu, &request)?;
            }
            Ok::<_, SimError>(())
        };
        try_join!(store, load, driver)?;
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn cancellation_after_issue_holds_slot_until_reply() {
    let mut engine = Engine::default();
    let lsu = load_store_unit_fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        let mut transfer = Box::pin(lsu.access_ranges(
            AccessType::ReadRequest,
            once(8..16),
            &lanes,
            "load",
            &group,
        ));
        assert!(poll!(&mut transfer).is_pending());
        lsu.clock.wait_ticks(1)?.await;
        assert!(poll!(&mut transfer).is_pending());
        let request = wait_for_issued_request(&lsu).await?;
        drop(transfer);
        lsu.clock.wait_ticks(5)?.await;
        assert!(lsu.state.active_request_slots.borrow()[0].in_use);
        assert_eq!(lsu.state.request_arbiter.count(), 0);
        deliver_memory_response(&lsu, &request)?;
        lsu.clock.wait_ticks(1)?.await;
        assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn unmapped_access_range_is_rejected() {
    let mut engine = Engine::default();
    let lsu = load_store_unit_fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        let unmapped_start = MEMORY_CAPACITY_BYTES as u128;
        let error = lsu
            .access_ranges(
                AccessType::ReadRequest,
                once(unmapped_start..unmapped_start + LSU_ACCESS_BYTES as u128),
                &lanes,
                "load",
                &group,
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Unmapped LSU address"),
            "{error}"
        );
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn cancellation_before_issue_releases_slot_and_request_turn() {
    let mut engine = Engine::default();
    let lsu = load_store_unit_fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        // Cancel during the END-phase wait, after acquiring a request slot.
        let mut transfer = Box::pin(lsu.access_ranges(
            AccessType::ReadRequest,
            once(0..8),
            &lanes,
            "load",
            &group,
        ));
        assert!(poll!(&mut transfer).is_pending());
        assert!(lsu.state.active_request_slots.borrow()[0].in_use);
        drop(transfer);
        assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
        assert_eq!(lsu.state.request_arbiter.count(), 0);

        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn cancelled_waiter_does_not_release_held_request_turn() {
    let mut engine = Engine::default();
    let lsu = load_store_unit_fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        // The waiting transfer does not own the held request turn.
        let held = lsu.state.request_arbiter.request().await;
        let mut blocked = Box::pin(lsu.access_ranges(
            AccessType::ReadRequest,
            once(0..8),
            &lanes,
            "load",
            &group,
        ));
        assert!(poll!(&mut blocked).is_pending());
        drop(blocked);
        assert_eq!(lsu.state.request_arbiter.count(), 1);
        drop(held);
        assert_eq!(lsu.state.request_arbiter.count(), 0);
        assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
        Ok(())
    });
    engine.run().unwrap();
}
