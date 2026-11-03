// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::future::{AbortHandle, Abortable, Either, select};
use gwr_engine::events::once::Once;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::Event;
use gwr_resources::base::{Resource, ResourceGuard};

#[test]
fn resource_empty() {
    const CAPACITY: usize = 2;
    const NUM_RESOURCE_REQUESTS: usize = 5;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let resource = Resource::new(CAPACITY);

    for i in 0..NUM_RESOURCE_REQUESTS {
        let clock = clock.clone();
        let resource = resource.clone();
        engine.spawn(async move {
            println!("RESOURCE REQUEST {i} start @ {}", clock.tick_now());
            resource.request().await;
            clock.wait_ticks(10)?.await;
            println!("RESOURCE REQUEST {i} done @ {}", clock.tick_now());
            resource.release().await?;
            println!("RESOURCE RELEASE {i} @ {}", clock.tick_now());
            Ok(())
        });
    }

    engine.run().unwrap();

    assert_eq!(resource.count(), 0);
}

#[test]
#[should_panic(
    expected = "called `Result::unwrap()` on an `Err` value: SimError(\"Invalid release\")"
)]
fn resource_more_releases() {
    const CAPACITY: usize = 2;
    const NUM_RESOURCE_REQUESTS: usize = 3;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let resource = Resource::new(CAPACITY);

    for i in 0..NUM_RESOURCE_REQUESTS {
        let clock = clock.clone();
        let resource = resource.clone();
        engine.spawn(async move {
            println!("RESOURCE REQUEST {i} start @ {}", clock.tick_now());
            resource.request().await;
            clock.wait_ticks(10)?.await;
            println!("RESOURCE REQUEST {i} done @ {}", clock.tick_now());
            resource.release().await?;
            println!("RESOURCE RELEASE {i} @ {}", clock.tick_now());
            resource.release().await?;
            Ok(())
        });
    }

    engine.run().unwrap();
}

#[test]
fn resource_no_release() {
    const CAPACITY: usize = 2;
    const NUM_RESOURCE_REQUESTS: usize = 5;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let resource = Resource::new(CAPACITY);

    for i in 0..NUM_RESOURCE_REQUESTS {
        let clock = clock.clone();
        let resource = resource.clone();
        engine.spawn(async move {
            println!("RESOURCE REQUEST {i} start @ {}", clock.tick_now());
            resource.request().await;
            clock.wait_ticks(10)?.await;
            println!("RESOURCE REQUEST {i} done @ {}", clock.tick_now());
            Ok(())
        });
    }

    engine.run().unwrap();

    assert_eq!(resource.count(), CAPACITY);
}

#[test]
fn resource_guard() {
    const CAPACITY: usize = 2;
    const NUM_RESOURCE_REQUESTS: usize = 5;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let resource = Resource::new(CAPACITY);

    for i in 0..NUM_RESOURCE_REQUESTS {
        let clock = clock.clone();
        let resource = resource.clone();
        engine.spawn(async move {
            println!("RESOURCE GUARD {i} start @ {}", clock.tick_now());
            ResourceGuard::new(resource).await;
            clock.wait_ticks(10)?.await;
            println!("RESOURCE GUARD {i} done @ {}", clock.tick_now());
            Ok(())
        });
    }

    engine.run().unwrap();

    assert_eq!(resource.count(), 0);
}

/// Cancelling a guard acquisition while it waits must leave the existing
/// request's slot occupied until that request releases it.
#[test]
fn cancelling_guard_acquisition_does_not_release_held_capacity() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let resource = Resource::new(1);
    let completed = Rc::new(Cell::new(false));

    {
        let resource = resource.clone();
        let completed = completed.clone();
        engine.spawn(async move {
            // Acquire the only slot, so creating a guard must wait.
            resource.request().await;
            let acquisition = Box::pin(ResourceGuard::new(resource.clone()));

            // Wait for either the guard acquisition or one clock tick to
            // complete. The slot stays held, so the clock wait finishes first
            // and lets us cancel the still-pending acquisition by dropping it.
            match select(acquisition, clock.wait_ticks(1)?).await {
                Either::Left(_) => panic!("guard acquisition should wait while the slot is held"),
                Either::Right(((), acquisition)) => drop(acquisition),
            }

            // Cancellation must leave the original request's slot occupied.
            assert_eq!(resource.count(), 1);
            resource.release().await?;
            completed.set(true);
            Ok(())
        });
    }

    engine.run().unwrap();
    assert!(completed.get(), "guard acquisition was not cancelled");
    assert_eq!(resource.count(), 0);
}

/// Cancelling a queued request must let the next waiter acquire the slot when
/// the current holder releases it.
#[test]
fn cancelling_pending_request_does_not_block_next_request() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let resource = Resource::new(1);
    let cancelled = Rc::new(Cell::new(false));
    let acquired = Rc::new(Cell::new(false));

    {
        let resource = resource.clone();
        let clock = clock.clone();
        engine.spawn(async move {
            // Hold the slot until both waiting requests have had time to queue.
            resource.request().await;
            clock.wait_ticks(4)?.await;
            resource.release().await?;
            Ok(())
        });
    }

    {
        let resource = resource.clone();
        let clock = clock.clone();
        let cancelled = cancelled.clone();
        engine.spawn(async move {
            clock.wait_ticks(1)?.await;

            // Queue a second request, then cancel it at tick 2.
            match select(resource.request(), clock.wait_ticks(1)?).await {
                Either::Left(_) => panic!("request should wait while the slot is held"),
                Either::Right(((), request)) => drop(request),
            }
            assert_eq!(resource.count(), 1);
            cancelled.set(true);
            Ok(())
        });
    }

    {
        let resource = resource.clone();
        let acquired = acquired.clone();
        engine.spawn(async move {
            // Queue a third request before the original holder releases.
            clock.wait_ticks(3)?.await;
            resource.request().await;
            assert_eq!(clock.tick_now().tick(), 4);
            acquired.set(true);
            resource.release().await?;
            Ok(())
        });
    }

    engine.run().unwrap();
    assert!(cancelled.get(), "the second request was not cancelled");
    assert!(acquired.get(), "the third request was left waiting");
    assert_eq!(resource.count(), 0);
}

/// Cancelling the notified queue head must let its successors use every free
/// slot. Checks one and two free slots.
#[test]
fn cancelling_requests_after_release_wakes_successors_for_each_free_slot() {
    for capacity in [1, 2] {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let resource = Resource::new(capacity);
        let cancelled = Rc::new(Cell::new(0));
        let acquired = Rc::new(Cell::new(0));
        let all_acquired = Once::default();
        let mut abort_handles = Vec::new();

        for _ in 0..capacity {
            let (abort_handle, registration) = AbortHandle::new_pair();
            abort_handles.push(abort_handle);
            let resource = resource.clone();
            let clock = clock.clone();
            let cancelled = cancelled.clone();
            engine.spawn(async move {
                // These requests queue first. The head is notified on release.
                clock.wait_ticks(1)?.await;
                assert!(
                    Abortable::new(resource.request(), registration)
                        .await
                        .is_err()
                );
                cancelled.set(cancelled.get() + 1);
                Ok(())
            });
        }

        for _ in 0..capacity {
            let resource = resource.clone();
            let clock = clock.clone();
            let acquired = acquired.clone();
            let all_acquired = all_acquired.clone();
            engine.spawn(async move {
                // These successors need the cancelled requests to pass on their
                // wakeups.
                clock.wait_ticks(2)?.await;
                let _guard = ResourceGuard::new(resource.clone()).await;
                acquired.set(acquired.get() + 1);
                if acquired.get() == capacity {
                    assert_eq!(resource.count(), capacity);
                    all_acquired.notify()?;
                }

                // Hold every acquired slot until all successors have acquired.
                // Otherwise one successor could release and hide a missing
                // wakeup.
                all_acquired.listen().await;
                Ok(())
            });
        }

        {
            let resource = resource.clone();
            engine.spawn(async move {
                // Acquire the resources, and then wait until the above tasks
                // have issued their requests.
                let mut guards = Vec::new();
                for _ in 0..capacity {
                    guards.push(ResourceGuard::new(resource.clone()).await);
                }
                clock.wait_ticks(3)?.await;

                // Release every slot and wake the abortable requests,
                // scheduling their tasks.
                drop(guards);

                // Mark those requests as cancelled before their tasks can
                // resume.
                for abort_handle in abort_handles {
                    abort_handle.abort();
                }
                Ok(())
            });
        }

        engine.run().unwrap();
        assert_eq!(cancelled.get(), capacity, "capacity={capacity}");
        assert_eq!(
            acquired.get(),
            capacity,
            "successors left waiting; capacity={capacity}"
        );
        assert_eq!(resource.count(), 0);
    }
}

/// A request already waiting must acquire a released slot before the original
/// holder can acquire it again.
#[test]
fn waiting_request_acquires_before_reacquisition() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let resource = Resource::new(1);
    let holder_completed = Rc::new(Cell::new(false));
    let waiter_completed = Rc::new(Cell::new(false));

    {
        let resource = resource.clone();
        let clock = clock.clone();
        let holder_completed = holder_completed.clone();
        engine.spawn(async move {
            let guard = ResourceGuard::new(resource.clone()).await;
            clock.wait_ticks(2)?.await;
            drop(guard);

            // Reacquiring must wait behind the request that queued at tick 1.
            let _guard = ResourceGuard::new(resource).await;
            assert_eq!(clock.tick_now().tick(), 3);
            holder_completed.set(true);
            Ok(())
        });
    }

    {
        let resource = resource.clone();
        let waiter_completed = waiter_completed.clone();
        engine.spawn(async move {
            clock.wait_ticks(1)?.await;
            let _guard = ResourceGuard::new(resource).await;

            // This request gets the slot at tick 2 and holds it until tick 3.
            assert_eq!(clock.tick_now().tick(), 2);
            clock.wait_ticks(1)?.await;
            waiter_completed.set(true);
            Ok(())
        });
    }

    engine.run().unwrap();
    assert!(
        holder_completed.get(),
        "the replacement holder did not finish"
    );
    assert!(
        waiter_completed.get(),
        "the notified request was left waiting"
    );
    assert_eq!(resource.count(), 0);
}

/// Requests must acquire in queue order, even when several slots are released
/// together and a new request tries to take one before the waiters resume.
#[test]
fn queued_requests_acquire_in_fifo_order_for_each_capacity() {
    for capacity in [1, 2] {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let resource = Resource::new(capacity);
        let order = Rc::new(RefCell::new(Vec::new()));
        let completed = Rc::new(Cell::new(0));

        for index in 0..3_u64 {
            let resource = resource.clone();
            let clock = clock.clone();
            let order = order.clone();
            let completed = completed.clone();
            engine.spawn(async move {
                // Join the queue at ticks 1, 2 and 3, in that order.
                clock.wait_ticks(index + 1)?.await;
                let _guard = ResourceGuard::new(resource).await;
                order.borrow_mut().push(index);
                clock.wait_ticks(1)?.await;
                completed.set(completed.get() + 1);
                Ok(())
            });
        }

        {
            let resource = resource.clone();
            let order = order.clone();
            let completed = completed.clone();
            engine.spawn(async move {
                let mut guards = Vec::new();
                for _ in 0..capacity {
                    guards.push(ResourceGuard::new(resource.clone()).await);
                }
                clock.wait_ticks(4)?.await;

                // Release every slot, then try to acquire before the queued
                // tasks resume.
                drop(guards);
                let _guard = ResourceGuard::new(resource).await;
                order.borrow_mut().push(3);
                completed.set(completed.get() + 1);
                Ok(())
            });
        }

        engine.run().unwrap();
        assert_eq!(*order.borrow(), [0, 1, 2, 3], "capacity={capacity}");
        assert_eq!(
            completed.get(),
            4,
            "requests left waiting; capacity={capacity}"
        );
        assert_eq!(resource.count(), 0);
    }
}
