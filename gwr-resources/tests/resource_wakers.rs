// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Wake, Waker};

use gwr_resources::base::Resource;

// Direct polling lets these tests supply distinct wakers and inspect exactly
// which queued wakers are retained and notified, independently of the engine's
// executor.
#[derive(Default)]
struct WakeCount(AtomicUsize);

impl WakeCount {
    fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Cancelling a queued request drops its stored waker, so only the next waiter
/// wakes.
#[test]
fn cancelling_pending_request_removes_its_waker() {
    let resource = Resource::new(1);

    // Create a request for the resource - this should succeed.
    let mut cx = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut resource.request()).poll(&mut cx).is_ready());

    // Create a second request for the resource - this should be made to wait.
    let cancelled_wakes = Arc::new(WakeCount::default());
    let cancelled_waker = Waker::from(cancelled_wakes.clone());
    let mut cancelled_cx = Context::from_waker(&cancelled_waker);
    let mut cancelled = resource.request();
    assert!(
        Pin::new(&mut cancelled)
            .poll(&mut cancelled_cx)
            .is_pending()
    );

    // Create a third request for the resource - this should be made to wait.
    let next_wakes = Arc::new(WakeCount::default());
    let next_waker = Waker::from(next_wakes.clone());
    let mut next_cx = Context::from_waker(&next_waker);
    let mut next = resource.request();
    assert!(Pin::new(&mut next).poll(&mut next_cx).is_pending());

    // Cancel the second request and confirm its queued waker has been dropped.
    drop(cancelled);
    // Only cancelled_wakes and cancelled_waker retain strong references now.
    assert_eq!(Arc::strong_count(&cancelled_wakes), 2);

    // Release the occupied slot; only the third request should be woken.
    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
    assert_eq!(cancelled_wakes.count(), 0);
    assert_eq!(next_wakes.count(), 1);

    // After the notification, poll the request to actually acquire the slot.
    assert!(Pin::new(&mut next).poll(&mut next_cx).is_ready());
    assert_eq!(resource.count(), 1);
    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
}

/// Polling again uses the latest waker without duplicating the request or
/// moving its place in the queue.
#[test]
fn pending_request_replaces_its_waker_without_changing_queue_order() {
    let resource = Resource::new(1);
    let mut cx = Context::from_waker(Waker::noop());

    // Occupy the only slot so both requests below must queue.
    assert!(Pin::new(&mut resource.request()).poll(&mut cx).is_ready());

    // Queue the first request with a waker that counts its notifications.
    let old_wakes = Arc::new(WakeCount::default());
    let old_waker = Waker::from(old_wakes.clone());
    let mut first = resource.request();
    assert!(
        Pin::new(&mut first)
            .poll(&mut Context::from_waker(&old_waker))
            .is_pending()
    );

    // Queue a second request behind it.
    let second_wakes = Arc::new(WakeCount::default());
    let second_waker = Waker::from(second_wakes.clone());
    let mut second = resource.request();
    assert!(
        Pin::new(&mut second)
            .poll(&mut Context::from_waker(&second_waker))
            .is_pending()
    );

    // Poll the first request with a new waker, keeping its place in the queue.
    let new_wakes = Arc::new(WakeCount::default());
    let new_waker = Waker::from(new_wakes.clone());
    let mut new_cx = Context::from_waker(&new_waker);
    assert!(Pin::new(&mut first).poll(&mut new_cx).is_pending());

    // Releasing must notify only the first request's replacement waker.
    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
    assert_eq!(old_wakes.count(), 0);
    assert_eq!(new_wakes.count(), 1);
    assert_eq!(second_wakes.count(), 0);

    // Complete and drop the request; no duplicate queue entry should remain.
    assert!(Pin::new(&mut first).poll(&mut new_cx).is_ready());
    drop(first);

    // The next release should wake the second waiter, not notify the first
    // again.
    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
    assert_eq!(new_wakes.count(), 1);
    assert_eq!(second_wakes.count(), 1);
    assert!(
        Pin::new(&mut second)
            .poll(&mut Context::from_waker(&second_waker))
            .is_ready()
    );
    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
}

/// A later waiter cannot take a released slot before the queue head is polled.
#[test]
fn later_waiter_cannot_acquire_before_queue_head() {
    let resource = Resource::new(1);
    let mut cx = Context::from_waker(Waker::noop());

    // Occupy the only slot before either waiter starts.
    assert!(Pin::new(&mut resource.request()).poll(&mut cx).is_ready());

    // Poll in this order to queue first ahead of second.
    let mut first = resource.request();
    let mut second = resource.request();
    assert!(Pin::new(&mut first).poll(&mut cx).is_pending());
    assert!(Pin::new(&mut second).poll(&mut cx).is_pending());

    // Free the slot, then deliberately poll the later waiter first.
    // It must stay pending and leave the slot available for the queue head.
    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
    assert!(Pin::new(&mut second).poll(&mut cx).is_pending());
    assert_eq!(resource.count(), 0);

    // The head acquires first; its release then lets the second request
    // acquire.
    assert!(Pin::new(&mut first).poll(&mut cx).is_ready());
    assert_eq!(resource.count(), 1);

    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
    assert!(Pin::new(&mut second).poll(&mut cx).is_ready());
    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
}

/// Cancelling a notified head wakes the next waiter without retaining its
/// waker.
#[test]
fn cancelling_queue_head_with_free_capacity_wakes_next() {
    let resource = Resource::new(1);
    let mut cx = Context::from_waker(Waker::noop());

    // Occupy the slot, then queue two requests with separate notification
    // counters.
    assert!(Pin::new(&mut resource.request()).poll(&mut cx).is_ready());

    let first_wakes = Arc::new(WakeCount::default());
    let first_waker = Waker::from(first_wakes.clone());
    let mut first = resource.request();
    assert!(
        Pin::new(&mut first)
            .poll(&mut Context::from_waker(&first_waker))
            .is_pending()
    );

    let next_wakes = Arc::new(WakeCount::default());
    let next_waker = Waker::from(next_wakes.clone());
    let mut next_cx = Context::from_waker(&next_waker);
    let mut next = resource.request();
    assert!(Pin::new(&mut next).poll(&mut next_cx).is_pending());

    // Releasing notifies the head, leaving its successor asleep.
    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
    assert_eq!(next_wakes.count(), 0);

    // Cancelling before acquisition must wake the next waiter for the free
    // slot.
    drop(first);

    // Only first_wakes and first_waker still own references to this counter.
    assert_eq!(Arc::strong_count(&first_wakes), 2);
    assert_eq!(next_wakes.count(), 1);

    // Poll the notified successor to acquire the slot, then release it
    // normally.
    assert!(Pin::new(&mut next).poll(&mut next_cx).is_ready());
    assert_eq!(resource.count(), 1);
    assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
}

/// Acquiring at the queue head wakes the next waiter while another slot is
/// free.
#[test]
fn queue_head_acquisition_chains_wakes_for_free_slots() {
    let resource = Resource::new(2);
    let mut cx = Context::from_waker(Waker::noop());

    // Occupy both slots so the following requests must queue.
    for _ in 0..2 {
        assert!(Pin::new(&mut resource.request()).poll(&mut cx).is_ready());
    }

    // Queue two waiters; count notifications sent to the second one.
    let mut first = resource.request();
    assert!(Pin::new(&mut first).poll(&mut cx).is_pending());
    let second_wakes = Arc::new(WakeCount::default());
    let second_waker = Waker::from(second_wakes.clone());
    let mut second_cx = Context::from_waker(&second_waker);
    let mut second = resource.request();
    assert!(Pin::new(&mut second).poll(&mut second_cx).is_pending());

    // Free both slots without polling either waiter. Only the head is notified.
    for _ in 0..2 {
        assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
    }
    let wakes_before_acquisition = second_wakes.count();

    // Acquiring one slot must wake the successor to use the other free slot.
    assert!(Pin::new(&mut first).poll(&mut cx).is_ready());
    assert_eq!(resource.count(), 1);
    assert!(second_wakes.count() > wakes_before_acquisition);

    // Both waiters must be able to hold slots together, without an intervening
    // release.
    assert!(Pin::new(&mut second).poll(&mut second_cx).is_ready());
    assert_eq!(resource.count(), 2);

    // Release both successful acquisitions.
    for _ in 0..2 {
        assert!(Pin::new(&mut resource.release()).poll(&mut cx).is_ready());
    }
}
