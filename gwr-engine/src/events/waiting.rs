// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::task::Waker;

pub(super) struct Waiting {
    listeners: RefCell<VecDeque<ListenerWaker>>,
    next_listener_id: Cell<u64>,
}

struct ListenerWaker {
    id: u64,
    waker: Waker,
}

impl Waiting {
    pub(super) fn new() -> Self {
        Self {
            listeners: RefCell::new(VecDeque::new()),
            next_listener_id: Cell::new(0),
        }
    }

    pub(super) fn register_listener(&self, waker: Waker) -> u64 {
        let listener_id = self.next_listener_id.get();
        self.next_listener_id.set(listener_id + 1);
        self.listeners.borrow_mut().push_back(ListenerWaker {
            id: listener_id,
            waker,
        });
        listener_id
    }

    pub(super) fn remove_listener(&self, listener_id: u64) {
        // Get the listener, if there is one
        let mut listeners = self.listeners.borrow_mut();
        let index = listeners
            .iter()
            .position(|listener| listener.id == listener_id);
        let listener = index.and_then(|i| listeners.remove(i));

        // Drop the listener after releasing the borrow
        drop(listeners);
        drop(listener);
    }

    pub(super) fn wake_all(&self) {
        // Release the borrow before waking. A wake can drop a cancelled
        // task, whose future may remove itself from the listener queue.
        std::iter::from_fn(|| self.listeners.borrow_mut().pop_front())
            .for_each(|listener| listener.waker.wake());
    }
}

impl Default for Waiting {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use futures::task::noop_waker;

    use super::*;

    #[test]
    fn default_creates_empty_waiting_list() {
        let waiting = Waiting::default();

        assert!(waiting.listeners.borrow().is_empty());
        assert_eq!(waiting.next_listener_id.get(), 0);
    }

    #[test]
    fn removing_unknown_listener_is_a_noop() {
        let waiting = Waiting::new();

        waiting.remove_listener(7);

        assert!(waiting.listeners.borrow().is_empty());
        assert_eq!(waiting.next_listener_id.get(), 0);

        let listener_id = waiting.register_listener(noop_waker());
        assert_eq!(waiting.listeners.borrow().len(), 1);
        assert_eq!(waiting.next_listener_id.get(), 1);

        waiting.remove_listener(listener_id);

        assert!(waiting.listeners.borrow().is_empty());
        assert_eq!(waiting.next_listener_id.get(), 1);
    }
}
