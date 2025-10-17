// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use gwr_engine::sim_error;
use gwr_engine::types::SimResult;

struct ResourceState {
    // Number of requests that can be handled concurrently
    capacity: usize,

    // Current number of concurrent requests
    count: usize,

    // Waiting requests
    queue: VecDeque<Waker>,
}

pub struct ResourceRequest {
    shared_state: Rc<RefCell<ResourceState>>,
}

pub struct ResourceRelease {
    shared_state: Rc<RefCell<ResourceState>>,
}

impl ResourceState {
    pub fn release(&mut self) -> SimResult {
        if self.count == 0 {
            return sim_error!("Invalid release");
        }
        self.count -= 1;
        if let Some(p) = self.queue.pop_front() {
            p.wake();
        }
        Ok(())
    }
}

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
                queue: VecDeque::new(),
            })),
        }
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn request(&self) -> ResourceRequest {
        ResourceRequest {
            shared_state: self.shared_state.clone(),
        }
    }

    #[must_use = "Futures do nothing unless you `.await` or otherwise use them"]
    pub fn release(&self) -> ResourceRelease {
        ResourceRelease {
            shared_state: self.shared_state.clone(),
        }
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.shared_state.borrow().count
    }
}

impl Future for ResourceRequest {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.shared_state.borrow().count == self.shared_state.borrow().capacity {
            self.shared_state
                .borrow_mut()
                .queue
                .push_back(cx.waker().clone());
            Poll::Pending
        } else {
            self.shared_state.borrow_mut().count += 1;
            Poll::Ready(())
        }
    }
}

impl Future for ResourceRelease {
    type Output = SimResult;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.shared_state.borrow_mut().release()?;
        Poll::Ready(Ok(()))
    }
}

pub struct ResourceGuard {
    obj: Resource,
}

impl ResourceGuard {
    pub async fn new(resource: Resource) -> Self {
        let guard = Self {
            obj: resource.clone(),
        };
        guard.obj.request().await;
        guard
    }
}

impl Drop for ResourceGuard {
    fn drop(&mut self) {
        self.obj.shared_state.borrow_mut().release().unwrap();
    }
}
