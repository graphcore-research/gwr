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
            // Ensure the listener isn't dropped until the mutable borrow is
            // released.
            let listener = self.listeners.borrow_mut().remove(index);
            drop(listener);
        }
    }

    pub(super) fn wake_all(&self) {
        // This is often called when some resource handle is dropped, to notify
        // waiters that the resource is now available.
        //
        // If those wakers have been cancelled (e.g. because the simulation
        // engine is being dropped), this may lead to them immediately dropping
        // the resource request and removing themselves as a listener - i.e.
        // attempting another mutable borrow of the listener queue.

        // Just take ownership of the current listener queue, to prevent
        // recursive borrows.
        let mut current_listeners = std::mem::take(&mut *self.listeners.borrow_mut());
        for listener in current_listeners.drain(..) {
            listener.waker.wake();
        }

        // If nothing registered a new listener, then swap the old queue back in
        // to reuse the allocated memory.
        let mut new_listeners = self.listeners.borrow_mut();
        if new_listeners.is_empty() {
            *new_listeners = current_listeners;
        }
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
