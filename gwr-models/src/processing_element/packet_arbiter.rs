// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! FIFO, cancellation-safe packet turns. One queued turn per live producer
//! gives round-robin issue without eagerly materializing transfer packets.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gwr_engine::events::repeated::Repeated;
use gwr_engine::traits::Event;

#[derive(Clone, Default)]
pub(super) struct PacketArbiter(Rc<Inner>);
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
impl PacketArbiter {
    pub(super) async fn request(&self) -> PacketTurn {
        let id = {
            let mut s = self.0.state.borrow_mut();
            let id = s.next;
            s.next = id.checked_add(1).expect("packet ticket overflow");
            s.queue.push_back(id);
            id
        };
        let mut waiter = Waiter {
            arbiter: self.clone(),
            id,
            queued: true,
        };
        loop {
            {
                let mut s = self.0.state.borrow_mut();
                if !s.busy && s.queue.front() == Some(&id) {
                    s.queue.pop_front();
                    s.busy = true;
                    waiter.queued = false;
                    return PacketTurn(self.clone());
                }
            }
            self.0.changed.listen().await;
        }
    }
    #[cfg(test)]
    pub(super) fn count(&self) -> usize {
        usize::from(self.0.state.borrow().busy)
    }
}
struct Waiter {
    arbiter: PacketArbiter,
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
pub(super) struct PacketTurn(PacketArbiter);
impl Drop for PacketTurn {
    fn drop(&mut self) {
        self.0.0.state.borrow_mut().busy = false;
        self.0.0.changed.notify();
    }
}
