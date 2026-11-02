// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::collections::BTreeMap;
use std::iter::once;

use futures::{poll, try_join};

use super::*;
use crate::memory::traits::AccessMemory;
use crate::processing_element::address_space::AddressRegionPolicy;
use crate::processing_element::compute::ComputeConfig;

const LSU_ACCESS_BYTES: usize = 64;
const MEMORY_CAPACITY_BYTES: u64 = 256;

fn load_store_unit_fixture(engine: &mut Engine) -> Rc<LoadStoreUnit> {
    let mut map = AddressSpace::new(LSU_ACCESS_BYTES as u64).unwrap();
    map.insert(
        0,
        MEMORY_CAPACITY_BYTES,
        AddressRegionPolicy::Passthrough {
            device: DeviceId(0),
        },
    )
    .unwrap();
    load_store_unit_with_address_space(engine, map)
}

fn load_store_unit_with_address_space(engine: &mut Engine, map: AddressSpace) -> Rc<LoadStoreUnit> {
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
            address_space: Rc::new(map),
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
    let mut map = AddressSpace::new(access_bytes as u64)?;
    map.insert(
        start,
        bytes,
        AddressRegionPolicy::Passthrough {
            device: DeviceId(0),
        },
    )?;
    let config = ProcessingElementConfig {
        num_active_requests: 1,
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

    assert!(error.to_string().contains("access size"), "{error}");
}

#[test]
fn rejects_a_memory_region_end_inside_an_lsu_block() {
    const REGION_BYTES: u64 = MEMORY_CAPACITY_BYTES - LSU_ACCESS_BYTES as u64 / 2;
    let mut engine = Engine::default();

    let error = register_lsu_with_memory_region(&mut engine, 0, REGION_BYTES, LSU_ACCESS_BYTES)
        .unwrap_err();

    assert!(error.to_string().contains("access size"), "{error}");
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

#[test]
fn virtual_tensor_ranges_issue_translated_striped_packets() {
    const VIRTUAL_BASE: u64 = 0x1000;
    const BANK0_BASE: u64 = 0x8000;
    const BANK1_BASE: u64 = 0xa000;
    let mut engine = Engine::default();
    let mut space = AddressSpace::new(LSU_ACCESS_BYTES as u64).unwrap();
    space
        .insert(
            VIRTUAL_BASE,
            MEMORY_CAPACITY_BYTES,
            AddressRegionPolicy::Striped {
                device_size_bytes: MEMORY_CAPACITY_BYTES / 2,
                banks: vec![(DeviceId(4), BANK0_BASE), (DeviceId(5), BANK1_BASE)],
            },
        )
        .unwrap();
    let lsu = load_store_unit_with_address_space(&mut engine, space);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "striped"));
        // Partial first and last packets still use the address-space width.
        let transfer = lsu.access_ranges(
            AccessType::ReadRequest,
            once(u128::from(VIRTUAL_BASE + 8)..u128::from(VIRTUAL_BASE + 136)),
            &lanes,
            "load",
            &group,
        );
        let driver = async {
            for (device, address) in [
                (DeviceId(4), BANK0_BASE),
                (DeviceId(5), BANK1_BASE),
                (DeviceId(4), BANK0_BASE + LSU_ACCESS_BYTES as u64),
            ] {
                let request = wait_for_issued_request(&lsu).await?;
                assert_eq!(request.dst_device(), device);
                assert_eq!(request.dst_addr(), address);
                assert_eq!(request.access_size_bytes(), LSU_ACCESS_BYTES);
                deliver_memory_response(&lsu, &request)?;
            }
            Ok::<_, SimError>(())
        };
        try_join!(transfer, driver)?;
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn virtual_view_preflight_ignores_unmapped_strided_holes() {
    use crate::processing_element::tensors::Tensor;
    use crate::processing_element::tensors::dtype::DataType;

    let mut engine = Engine::default();
    let mut space = AddressSpace::new(LSU_ACCESS_BYTES as u64).unwrap();
    for start in [0x1000, 0x1080] {
        space
            .insert(
                start,
                LSU_ACCESS_BYTES as u64,
                AddressRegionPolicy::Passthrough {
                    device: DeviceId(0),
                },
            )
            .unwrap();
    }
    let lsu = load_store_unit_with_address_space(&mut engine, space);
    let tensor = Tensor::new(&[2, 128], &DataType::Int8, 0x1000).unwrap();
    let view = TensorView::new(tensor.clone(), &[2, 1], &[0, 0]).unwrap();
    lsu.validate_mapped_view(&view).unwrap();
    assert!(
        lsu.validate_mapped_view(&TensorView::new_full(tensor))
            .is_err()
    );
}
