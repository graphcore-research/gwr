// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::task::Waker;

pub(super) struct Waiting {
    listeners: RefCell<Vec<ListenerWaker>>,
    next_listener_id: Cell<u64>,
}

struct ListenerWaker {
    id: u64,
    waker: Waker,
}

impl Waiting {
    pub(super) fn new() -> Self {
        Self {
            listeners: RefCell::new(Vec::new()),
            next_listener_id: Cell::new(0),
        }
    }

    pub(super) fn register_listener(&self, waker: Waker) -> u64 {
        let listener_id = self.next_listener_id.get();
        self.next_listener_id.set(listener_id + 1);
        self.listeners.borrow_mut().push(ListenerWaker {
            id: listener_id,
            waker,
        });
        listener_id
    }

    pub(super) fn remove_listener(&self, listener_id: u64) {
        let index = self
            .listeners
            .borrow()
            .iter()
            .position(|listener| listener.id == listener_id);
        if let Some(index) = index {
            self.listeners.borrow_mut().remove(index);
        }
    }

    pub(super) fn wake_all(&self) {
        for listener in self.listeners.borrow_mut().drain(..) {
            listener.waker.wake();
        }
    }
}

impl Default for Waiting {
    fn default() -> Self {
        Self::new()
    }
}
