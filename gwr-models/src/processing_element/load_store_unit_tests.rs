// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use super::*;
use crate::memory::traits::AccessMemory;

fn fixture(engine: &mut Engine) -> Rc<LoadStoreUnit> {
    let mut map = MemoryMap::new();
    map.insert(0, 128, DeviceId(0)).unwrap();
    fixture_with_map(engine, map)
}

fn fixture_with_map(engine: &mut Engine, map: MemoryMap) -> Rc<LoadStoreUnit> {
    let entity = Rc::new(Entity::new(engine.top(), "lsu"));
    Rc::new(LoadStoreUnit {
        entity: entity.clone(),
        spawner: engine.spawner(),
        clock: engine.default_clock(),
        max_access_size_bytes: 64,
        rx: RefCell::new(None),
        tx: RefCell::new(None),
        atomic_request_receiver: RefCell::new(None),
        atomic_reply_receiver: RefCell::new(None),
        state: Rc::new(LsuState {
            entity,
            memory_map: Rc::new(map),
            device_id: DeviceId(1),
            overhead_size_bytes: 0,
            active_request_slots: RefCell::new(vec![ActiveRequestSlot::default()]),
            pending_request_indices: RefCell::new(VecDeque::new()),
            new_request: Repeated::new(()),
            slot_available: Repeated::new(()),
            serialiser: PacketArbiter::default(),
            issue: RefCell::new(LsuIssueSnapshot::default()),
        }),
    })
}

async fn issued(lsu: &LoadStoreUnit) -> Result<MemoryAccess, SimError> {
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

fn reply(lsu: &LoadStoreUnit, request: &MemoryAccess) -> SimResult {
    lsu.state
        .put_response_in_active_request_slot(MemoryAccess::new(
            &lsu.entity,
            AccessType::ReadResponse,
            request.access_size_bytes(),
            request.dst_addr(),
            request.src_addr(),
            DeviceId(1),
            DeviceId(0),
            0,
        ))
}

#[test]
fn waiting_read_issues_before_long_store_finishes() {
    let mut engine = Engine::default();
    let mut map = MemoryMap::new();
    map.insert(0, 1024, DeviceId(0)).unwrap();
    let lsu = fixture_with_map(&mut engine, map);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "fair"));
        let store = lsu.access_ranges_retaining(
            AccessType::WriteNonPostedRequest,
            std::iter::once(0..512),
            &lanes,
            "store",
            &group,
            Rc::new(()),
        );
        let load = async {
            lsu.clock.wait_ticks(1)?.await;
            lsu.access_ranges_retaining(
                AccessType::ReadRequest,
                std::iter::once(512..576),
                &lanes,
                "load",
                &group,
                Rc::new(()),
            )
            .await
        };
        let driver = async {
            let mut read_at = None;
            let mut store_addresses = Vec::new();
            for ordinal in 0..9 {
                let request = issued(&lsu).await?;
                if request.access_type() == AccessType::ReadRequest {
                    read_at = Some(ordinal);
                } else {
                    store_addresses.push(request.dst_addr());
                }
                reply(&lsu, &request)?;
                lsu.clock.wait_ticks(1)?.await;
            }
            assert!(read_at.unwrap() < 8, "read waited for every store packet");
            assert_eq!(store_addresses, (0..8).map(|i| i * 64).collect::<Vec<_>>());
            Ok::<_, SimError>(())
        };
        futures::try_join!(store, load, driver)?;
        let snapshot = lsu.issue_snapshot();
        assert_eq!(
            (snapshot.issued_read_packets, snapshot.issued_write_packets),
            (1, 8)
        );
        assert_eq!(snapshot.peak_outstanding_packets, 1);
        assert_eq!(snapshot.outstanding_packets, 0);
        assert!(
            lsu.state
                .active_request_slots
                .borrow()
                .iter()
                .all(|slot| !slot.in_use)
        );
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn ordinary_custody_keeps_64_byte_blocks() {
    let mut engine = Engine::default();
    let lsu = fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        let transfer = lsu.access_ranges_retaining(
            AccessType::ReadRequest,
            std::iter::once(8..16),
            &lanes,
            "load",
            &group,
            Rc::new(()),
        );
        futures::pin_mut!(transfer);
        assert!(futures::poll!(&mut transfer).is_pending());
        lsu.clock.wait_ticks(1)?.await;
        assert!(futures::poll!(&mut transfer).is_pending());
        let request = issued(&lsu).await?;
        assert_eq!((request.dst_addr(), request.access_size_bytes()), (0, 64));
        reply(&lsu, &request)?;
        transfer.await?;
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn delayed_reply_holds_slot_generation() {
    let mut engine = Engine::default();
    let lsu = fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        let owner = Rc::new(());
        let weak = Rc::downgrade(&owner);
        let mut transfer = Box::pin(lsu.access_ranges_retaining(
            AccessType::ReadRequest,
            std::iter::once(8..16),
            &lanes,
            "load",
            &group,
            owner,
        ));
        assert!(futures::poll!(&mut transfer).is_pending());
        lsu.clock.wait_ticks(1)?.await;
        assert!(futures::poll!(&mut transfer).is_pending());
        let request = issued(&lsu).await?;
        drop(transfer);
        lsu.clock.wait_ticks(5)?.await;
        assert!(weak.upgrade().is_some());
        assert!(lsu.state.active_request_slots.borrow()[0].in_use);
        assert_eq!(lsu.state.serialiser.count(), 0);
        reply(&lsu, &request)?;
        lsu.clock.wait_ticks(1)?.await;
        assert!(weak.upgrade().is_none());
        assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn source_vectors_remain_explicit_16_byte_packets() {
    let mut engine = Engine::default();
    let lsu = fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        let stats = Rc::new(RefCell::new(super::super::ProcessingElementStats::default()));
        let issues = [super::super::runner::streamed_matrix::SourceVectorIssue {
            address: 16,
            chunk: 0,
            register: 0,
            thread: 0,
            source_row: 0,
            k_column: 0,
            operand: super::super::runner::streamed_matrix::SourceOperand::A,
        }; 2];
        let transfer =
            lsu.access_source_issues(issues, &lanes, "source", &group, Rc::new(()), stats.clone());
        futures::pin_mut!(transfer);
        for _ in 0..2 {
            assert!(futures::poll!(&mut transfer).is_pending());
            lsu.clock.wait_ticks(1)?.await;
            assert!(futures::poll!(&mut transfer).is_pending());
            let request = issued(&lsu).await?;
            assert_eq!((request.dst_addr(), request.access_size_bytes()), (16, 16));
            reply(&lsu, &request)?;
            lsu.clock.wait_ticks(1)?.await;
        }
        transfer.await?;
        assert_eq!(stats.borrow().read_bytes, 32);
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn partial_issue_failure_drains_already_issued_accesses() {
    let mut engine = Engine::default();
    let lsu = fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        let owner = Rc::new(());
        let weak = Rc::downgrade(&owner);
        let mut transfer = Box::pin(lsu.access_ranges_retaining(
            AccessType::ReadRequest,
            [0..8, 256..264],
            &lanes,
            "load",
            &group,
            owner,
        ));
        assert!(futures::poll!(&mut transfer).is_pending());
        lsu.clock.wait_ticks(1)?.await;
        // The second range fails mapping, but the first real request must
        // drain.
        assert!(futures::poll!(&mut transfer).is_pending());
        let request = issued(&lsu).await?;
        assert!(weak.upgrade().is_some());
        assert!(lsu.state.active_request_slots.borrow()[0].in_use);
        lsu.clock.wait_ticks(5)?.await;
        assert!(weak.upgrade().is_some());
        assert!(lsu.state.active_request_slots.borrow()[0].in_use);
        reply(&lsu, &request)?;
        assert!(transfer.await.is_err());
        assert!(weak.upgrade().is_none());
        assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
        assert!(lsu.state.pending_request_indices.borrow().is_empty());
        assert_eq!(lsu.state.serialiser.count(), 0);
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn cancelled_before_issue_returns_only_acquired_credits() {
    let mut engine = Engine::default();
    let lsu = fixture(&mut engine);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        // Cancel during the END-phase wait, after acquiring a request slot.
        let owner = Rc::new(());
        let weak = Rc::downgrade(&owner);
        let mut transfer = Box::pin(lsu.access_ranges_retaining(
            AccessType::ReadRequest,
            std::iter::once(0..8),
            &lanes,
            "load",
            &group,
            owner,
        ));
        assert!(futures::poll!(&mut transfer).is_pending());
        assert!(lsu.state.active_request_slots.borrow()[0].in_use);
        drop(transfer);
        assert!(weak.upgrade().is_none());
        assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
        assert_eq!(lsu.state.serialiser.count(), 0);

        // A second transfer waiting for serialisation never owns that credit.
        let held = lsu.state.serialiser.request().await;
        let mut blocked = Box::pin(lsu.access_ranges_retaining(
            AccessType::ReadRequest,
            std::iter::once(0..8),
            &lanes,
            "load",
            &group,
            Rc::new(()),
        ));
        assert!(futures::poll!(&mut blocked).is_pending());
        drop(blocked);
        assert_eq!(lsu.state.serialiser.count(), 1);
        drop(held);
        assert_eq!(lsu.state.serialiser.count(), 0);
        assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn retained_read_and_nonposted_write_drain_real_ports_after_caller_drop() {
    use crate::memory::{Memory, MemoryConfig};
    for access_type in [AccessType::ReadRequest, AccessType::WriteNonPostedRequest] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let mut map = MemoryMap::new();
        map.insert(0, 128, DeviceId(0)).unwrap();
        let config = ProcessingElementConfig {
            num_active_requests: 1,
            lsu_access_bytes: 64,
            overhead_size_bytes: 0,
            sram_bytes: 128,
            boundary_payload_service: None,
            local_payload_service: None,
            block_execution: None,
            compute: yaml_serde::from_str("engines: {}").unwrap(),
            operation_classes: std::collections::BTreeMap::default(),
        };
        let lsu = LoadStoreUnit::new_and_register(
            &engine,
            &clock,
            engine.top(),
            None,
            &config,
            &Rc::new(map),
            DeviceId(1),
        )
        .unwrap();
        let memory = Memory::<MemoryAccess>::new_and_register(
            &engine,
            &clock,
            engine.top(),
            "memory",
            MemoryConfig::new(0, 128, 64, 20),
        )
        .unwrap();
        lsu.connect_port_tx(memory.port_rx()).unwrap();
        memory.connect_port_tx(lsu.port_rx()).unwrap();
        engine.spawn(async move {
            let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
            let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
            let owner = Rc::new(("slot", 7usize));
            let weak = Rc::downgrade(&owner);
            let mut transfer = Box::pin(lsu.access_ranges_retaining(
                access_type,
                std::iter::once(8..16),
                &lanes,
                "transfer",
                &group,
                owner,
            ));
            assert!(futures::poll!(&mut transfer).is_pending());
            clock.wait_ticks(1)?.await;
            assert!(futures::poll!(&mut transfer).is_pending());
            drop(transfer);
            clock.wait_ticks(2)?.await;
            assert_eq!(*weak.upgrade().unwrap(), ("slot", 7));
            assert!(lsu.state.active_request_slots.borrow()[0].in_use);
            for _ in 0..100 {
                if weak.upgrade().is_none() {
                    break;
                }
                clock.wait_ticks(1)?.await;
            }
            assert!(weak.upgrade().is_none());
            assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
            assert_eq!(
                (memory.bytes_read(), memory.bytes_written()),
                if access_type == AccessType::ReadRequest {
                    (64, 0)
                } else {
                    (0, 64)
                }
            );
            Ok(())
        });
        engine.run().unwrap();
    }
}

#[test]
fn pipeline_preflight_rejects_aligned_packets_outside_direct_pe_map() {
    use crate::processing_element::tensors::dtype::DataType;
    use crate::processing_element::tensors::{Tensor, TensorView};
    let mut engine = Engine::default();
    let mut lsu = fixture(&mut engine);
    let mut map = MemoryMap::new();
    map.insert(8, 8, DeviceId(0)).unwrap();
    Rc::get_mut(&mut Rc::get_mut(&mut lsu).unwrap().state)
        .unwrap()
        .memory_map = Rc::new(map);
    let view = TensorView::new_full(Tensor::new(&[8], &DataType::Fp8, 8).unwrap());
    lsu.validate_mapped_view(&view).unwrap(); // Logical view is legal.
    assert!(
        lsu.validate_aligned_view(&view)
            .unwrap_err()
            .to_string()
            .contains("aligned")
    );
    assert_eq!(lsu.clock.tick_now().tick(), 0);
    assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
    assert!(lsu.state.pending_request_indices.borrow().is_empty());
}

#[test]
fn striped_cancelled_request_keeps_custody_until_reply() {
    use crate::memory::memory_map::MemoryRegion;
    let mut engine = Engine::default();
    let mut map = MemoryMap::new();
    map.insert_striped(
        vec![
            MemoryRegion {
                start: 0,
                end: 127,
                device: DeviceId(0),
            },
            MemoryRegion {
                start: 128,
                end: 255,
                device: DeviceId(2),
            },
        ],
        64,
    )
    .unwrap();
    let lsu = fixture_with_map(&mut engine, map);
    engine.spawn(async move {
        let lanes = Rc::new(RefCell::new(ActivityLanes::new(lsu.entity.clone(), "test")));
        let group = Rc::new(EntityGroup::new(&lsu.entity, "transfer"));
        let owner = Rc::new(());
        let weak = Rc::downgrade(&owner);
        let mut transfer = Box::pin(lsu.access_ranges_retaining(
            AccessType::ReadRequest,
            std::iter::once(64..128),
            &lanes,
            "load",
            &group,
            owner,
        ));
        assert!(futures::poll!(&mut transfer).is_pending());
        lsu.clock.wait_ticks(1)?.await;
        assert!(futures::poll!(&mut transfer).is_pending());
        let request = issued(&lsu).await?;
        assert_eq!(request.dst_addr(), 128);
        assert_eq!(request.dst_device(), DeviceId(2));
        drop(transfer);
        lsu.clock.wait_ticks(5)?.await;
        assert!(weak.upgrade().is_some());
        assert!(lsu.state.active_request_slots.borrow()[0].in_use);
        reply(&lsu, &request)?;
        lsu.clock.wait_ticks(1)?.await;
        assert!(weak.upgrade().is_none());
        assert!(!lsu.state.active_request_slots.borrow()[0].in_use);
        Ok(())
    });
    engine.run().unwrap();
}

#[test]
fn striped_source_vectors_validate_boundaries_and_actual_target_before_issue() {
    use super::super::runner::streamed_matrix::{SourceOperand, SourceVectorIssue};
    use crate::memory::memory_map::MemoryRegion;
    let mut engine = Engine::default();
    let mut map = MemoryMap::new();
    map.insert_striped(
        vec![
            MemoryRegion {
                start: 0,
                end: 127,
                device: DeviceId(0),
            },
            MemoryRegion {
                start: 128,
                end: 255,
                device: DeviceId(2),
            },
        ],
        64,
    )
    .unwrap();
    let lsu = fixture_with_map(&mut engine, map);
    let issue = SourceVectorIssue {
        address: 56,
        chunk: 0,
        register: 0,
        thread: 0,
        source_row: 0,
        k_column: 0,
        operand: SourceOperand::A,
    };
    assert!(
        lsu.validate_source_issues([issue])
            .unwrap_err()
            .to_string()
            .contains("stripe")
    );
    let issue = SourceVectorIssue {
        address: 64,
        ..issue
    };
    assert!(lsu.validate_source_target([issue], 0).is_err());
    lsu.validate_source_target([issue], 2).unwrap();
    assert!(lsu.state.pending_request_indices.borrow().is_empty());
    assert!(
        lsu.state
            .active_request_slots
            .borrow()
            .iter()
            .all(|slot| !slot.in_use)
    );
}

#[cfg(feature = "functional")]
#[test]
fn functional_issuer_rejects_its_own_striped_map() {
    use crate::memory::memory_map::MemoryRegion;
    let mut engine = Engine::default();
    let mut map = MemoryMap::new();
    map.insert_striped(
        vec![
            MemoryRegion {
                start: 0,
                end: 127,
                device: DeviceId(0),
            },
            MemoryRegion {
                start: 128,
                end: 255,
                device: DeviceId(1),
            },
        ],
        64,
    )
    .unwrap();
    let striped = fixture_with_map(&mut engine, map);
    assert!(
        striped
            .validate_functional_mapping()
            .unwrap_err()
            .to_string()
            .contains("striped")
    );
    let ordinary = fixture(&mut engine);
    ordinary.validate_functional_mapping().unwrap();
}
