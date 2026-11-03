// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};

struct ResourceState {
    // Number of requests that can be handled concurrently
    capacity: usize,

    // Number of acquired slots, excluding queued requests
    count: usize,

    // Next ID for a queued request; IDs are never reused within this resource.
    next_waiter_id: u64,

    // Waiting requests in FIFO order, retained until acquisition or
    // cancellation.
    queue: VecDeque<ResourceWaiter>,
}

struct ResourceWaiter {
    // Identifies the same queued request across polls and cancellation.
    id: u64,
    // Notification consumes the waker without removing the request's queue
    // entry.
    waker: Option<Waker>,
}

/// Acquires one slot when polled to completion. Dropping it while pending
/// cancels the wait; releasing an acquired slot is a separate operation.
pub struct ResourceRequest {
    shared_state: Rc<RefCell<ResourceState>>,
    waiter_id: Option<u64>,
}

/// Releases a previously acquired slot when polled.
pub struct ResourceRelease {
    shared_state: Rc<RefCell<ResourceState>>,
}

impl ResourceState {
    pub fn release(&mut self) -> Result<Option<Waker>, SimError> {
        if self.count == 0 {
            return sim_error!("Invalid release");
        }
        self.count -= 1;
        Ok(self.take_next_waker())
    }

    /// Take the queue head's waker if capacity is free.
    ///
    /// The entry stays until acquisition or cancellation to preserve FIFO
    /// order. Its `waker` becomes `None` to avoid duplicate notifications. The
    /// caller wakes it after releasing the shared-state borrow.
    fn take_next_waker(&mut self) -> Option<Waker> {
        if self.count < self.capacity {
            self.queue
                .front_mut()
                .and_then(|waiter| waiter.waker.take())
        } else {
            None
        }
    }

    // Remove the entry and return any stored waker to drop outside the borrow.
    fn remove_waiter(&mut self, id: u64) -> Option<Waker> {
        let index = self.queue.iter().position(|waiter| waiter.id == id)?;
        self.queue.remove(index).and_then(|waiter| waiter.waker)
    }
}

/// A resource with a fixed number of slots, acquired in the order requests are
/// first polled. Cancelling a pending request lets later requests keep
/// progressing.
#[derive(Clone)]
pub struct Resource {
    shared_state: Rc<RefCell<ResourceState>>,
}

impl Resource {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            shared_state: Rc::new(RefCell::new(ResourceState {
                capacity,
                count: 0,
                next_waiter_id: 0,
                queue: VecDeque::new(),
            })),
        }
    }

    /// Acquire a slot, waiting behind any requests that are already queued.
    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn request(&self) -> ResourceRequest {
        ResourceRequest {
            shared_state: self.shared_state.clone(),
            waiter_id: None,
        }
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn release(&self) -> ResourceRelease {
        ResourceRelease {
            shared_state: self.shared_state.clone(),
        }
    }

    /// Number of currently acquired slots.
    #[must_use]
    pub fn count(&self) -> usize {
        self.shared_state.borrow().count
    }
}

impl Future for ResourceRequest {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let request = self.get_mut();

        // Waker callbacks can reenter the resource, including when cloned or
        // dropped, so keep them outside the shared-state borrow.
        let waker = cx.waker().clone();
        let mut state = request.shared_state.borrow_mut();

        // Acquire only if a slot is free and this request is first in line.
        // An empty queue lets a new request acquire immediately.
        let can_acquire = state.count < state.capacity
            && state
                .queue
                .front()
                .is_none_or(|waiter| request.waiter_id == Some(waiter.id));

        let discarded_waker = if can_acquire {
            // Claim the slot; any existing queue head belongs to this request.
            state.count += 1;
            request.waiter_id = None;
            state.queue.pop_front().and_then(|waiter| waiter.waker)
        } else {
            // Keep one queue entry and ID per request, updating its waker on
            // later polls.
            let id = *request.waiter_id.get_or_insert_with(|| {
                let id = state.next_waiter_id;
                state.next_waiter_id = id.checked_add(1).expect("Resource waiter IDs exhausted");
                id
            });

            if let Some(waiter) = state.queue.iter_mut().find(|waiter| waiter.id == id) {
                waiter.waker.replace(waker)
            } else {
                state.queue.push_back(ResourceWaiter {
                    id,
                    waker: Some(waker),
                });
                None
            }
        };

        // Acquiring at the head may leave another slot free. Prepare to notify
        // the next waiter so it can acquire without waiting for another
        // release.
        let next_waker = state.take_next_waker();

        // Drop or wake saved wakers only after releasing the shared-state
        // borrow.
        drop(state);
        drop(discarded_waker);
        if let Some(waker) = next_waker {
            waker.wake();
        }

        if can_acquire {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for ResourceRequest {
    fn drop(&mut self) {
        let Some(id) = self.waiter_id.take() else {
            return;
        };
        let mut state = self.shared_state.borrow_mut();
        let removed_waker = state.remove_waiter(id);
        let next_waker = state.take_next_waker();
        // Release the borrow before dropping or waking any wakers.
        drop(state);
        drop(removed_waker);
        if let Some(waker) = next_waker {
            // Cancelling the head can leave a free slot with its successor
            // asleep. Ask that task to poll again so it gets a chance to
            // acquire.
            waker.wake();
        }
    }
}

impl Future for ResourceRelease {
    type Output = SimResult;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let next_waker = self.shared_state.borrow_mut().release()?;
        if let Some(waker) = next_waker {
            // Notify the waiting task to poll again now that capacity is free.
            waker.wake();
        }
        Poll::Ready(Ok(()))
    }
}

/// Holds one acquired slot and releases it when dropped.
pub struct ResourceGuard {
    resource: Resource,
}

impl ResourceGuard {
    pub async fn new(resource: Resource) -> Self {
        // Construct the guard only after acquisition, so cancelling this future
        // cannot release a slot held by another request.
        resource.request().await;
        Self { resource }
    }
}

impl Drop for ResourceGuard {
    fn drop(&mut self) {
        let next_waker = self.resource.shared_state.borrow_mut().release().unwrap();
        if let Some(waker) = next_waker {
            waker.wake();
        }
    }
}
