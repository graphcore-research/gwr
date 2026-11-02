// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! FIFO, cancellation-safe request turns. One queued turn per live transfer
//! gives round-robin issue without eagerly creating memory requests.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gwr_engine::events::repeated::Repeated;
use gwr_engine::traits::Event;

#[derive(Clone, Default)]
pub(super) struct RequestArbiter(Rc<Inner>);

impl RequestArbiter {
    pub(super) async fn request(&self) -> RequestTurn {
        let id = self.enqueue();
        let mut waiter = Waiter {
            arbiter: self.clone(),
            id,
            queued: true,
        };
        loop {
            if self.try_start_turn(id) {
                waiter.queued = false;
                return RequestTurn(self.clone());
            }
            self.0.changed.listen().await;
        }
    }

    fn enqueue(&self) -> u64 {
        let mut state = self.0.state.borrow_mut();
        let id = state.next;
        state.next = id.checked_add(1).expect("request ticket overflow");
        state.queue.push_back(id);
        id
    }

    fn try_start_turn(&self, id: u64) -> bool {
        let mut state = self.0.state.borrow_mut();
        if state.busy || state.queue.front() != Some(&id) {
            return false;
        }
        state.queue.pop_front();
        state.busy = true;
        true
    }

    #[cfg(test)]
    pub(super) fn count(&self) -> usize {
        usize::from(self.0.state.borrow().busy)
    }
}

struct Waiter {
    arbiter: RequestArbiter,
    id: u64,
    queued: bool,
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if self.queued {
            self.arbiter
                .0
                .state
                .borrow_mut()
                .queue
                .retain(|id| *id != self.id);
            self.arbiter.0.changed.notify();
        }
    }
}

pub(super) struct RequestTurn(RequestArbiter);

impl Drop for RequestTurn {
    fn drop(&mut self) {
        self.0.0.state.borrow_mut().busy = false;
        self.0.0.changed.notify();
    }
}

#[derive(Default)]
struct Inner {
    state: RefCell<State>,
    changed: Repeated<()>,
}

#[derive(Default)]
struct State {
    next: u64,
    queue: VecDeque<u64>,
    busy: bool,
}
