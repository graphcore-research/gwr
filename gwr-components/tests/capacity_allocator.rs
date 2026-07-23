// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::Cell;
use std::rc::Rc;

use futures::executor::block_on;
use gwr_components::capacity_allocator::CapacityAllocator;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;

#[test]
fn rejects_zero_capacity() {
    let engine = start_test(file!());

    let err = match CapacityAllocator::new(engine.top(), "alloc", 0, "bytes") {
        Ok(_) => panic!("zero-capacity allocator should fail"),
        Err(err) => err,
    };

    assert!(
        format!("{err}").contains("Unsupported CapacityAllocator with capacity of 0"),
        "Unexpected error message: {err}"
    );
}

#[test]
fn allocate_and_release_update_used_capacity() {
    let engine = start_test(file!());
    let allocator = CapacityAllocator::new(engine.top(), "alloc", 8, "bytes").unwrap();

    assert_eq!(allocator.used(), 0);
    assert!(allocator.has_capacity_for(8));

    allocator.allocate(3).unwrap();
    assert_eq!(allocator.used(), 3);
    assert!(allocator.has_capacity_for(5));
    assert!(!allocator.has_capacity_for(6));

    allocator.release(2);
    assert_eq!(allocator.used(), 1);

    allocator.release(1);
    assert_eq!(allocator.used(), 0);
}

#[test]
fn reject_allocation_larger_than_capacity() {
    let engine = start_test(file!());
    let allocator = CapacityAllocator::new(engine.top(), "alloc", 8, "bytes").unwrap();

    let err = allocator.check_units_can_fit(9).unwrap_err();

    assert!(
        format!("{err}").contains("Cannot allocate 9 bytes"),
        "Unexpected error message: {err}"
    );
}

#[test]
fn allocation_without_available_capacity_returns_error() {
    let engine = start_test(file!());
    let allocator = CapacityAllocator::new(engine.top(), "alloc", 4, "objects").unwrap();

    allocator.allocate(3).unwrap();

    let err = allocator.allocate(2).unwrap_err();

    assert!(
        format!("{err}").contains("Overflow"),
        "Unexpected error message: {err}"
    );
    assert_eq!(allocator.used(), 3);
}

#[test]
fn reservation_releases_capacity_when_dropped() {
    let engine = start_test(file!());
    let allocator = CapacityAllocator::new(engine.top(), "alloc", 8, "bytes").unwrap();

    {
        let _reservation = block_on(allocator.reserve(6)).unwrap();
        assert_eq!(allocator.used(), 6);
    }

    assert_eq!(allocator.used(), 0);
}

#[test]
fn reserve_waits_until_capacity_is_released() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let allocator = CapacityAllocator::new(engine.top(), "alloc", 4, "bytes").unwrap();

    allocator.allocate(4).unwrap();
    let reservation_done = Rc::new(Cell::new(false));

    {
        let allocator = allocator.clone();
        let reservation_done = reservation_done.clone();
        engine.spawn(async move {
            let _reservation = allocator.reserve(2).await?;
            reservation_done.set(true);
            Ok(())
        });
    }

    {
        let allocator = allocator.clone();
        let reservation_done = reservation_done.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            clock.wait_ticks(1).await;
            assert!(!reservation_done.get());
            assert_eq!(allocator.used(), 4);
            allocator.release(4);
            Ok(())
        });
    }

    run_simulation!(engine);

    assert!(reservation_done.get());
    assert_eq!(allocator.used(), 0);
    assert_eq!(clock.time_now_ns(), 1.0);
}
