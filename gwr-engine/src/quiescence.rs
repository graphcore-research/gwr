// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Opt-in, ordered work at executor quiescence.
//!
//! Producers request a callback rather than flushing during their own poll.
//! Requests coalesce until a snapshot is taken; requests made during a callback
//! belong to the next round. The executor drains runnable tasks between rounds.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::{Rc, Weak};

use crate::types::{SimError, SimResult};

/// Synchronous domain work and its full-run drain invariant.
pub trait QuiescenceCallback {
    /// Flush one batch without advancing simulation time. May wake tasks and
    /// request callbacks for the next round. Do not recursively run the engine.
    fn on_quiescence(&self) -> SimResult;

    /// Called only at natural executor termination, never on an intentional
    /// `run_until` stop. Report unfinished work with flow IDs, slots, blocked
    /// ports, dependencies, and expected response producers as applicable.
    ///
    /// The engine cannot infer domain liveness from parked futures. The domain
    /// owns this check; an observer must not mutate or schedule work here.
    fn check_drained(&self) -> SimResult {
        Ok(())
    }
}

struct Entry {
    id: u64,
    committed: Cell<bool>,
    callback: Rc<dyn QuiescenceCallback>,
    pending: Weak<RefCell<BTreeSet<u64>>>,
}

/// Cloneable notification handle. It does not keep the engine or callback
/// alive.
#[derive(Clone)]
pub struct QuiescenceHandle {
    entry: Weak<Entry>,
}

impl QuiescenceHandle {
    /// Request a callback at the next quiescent round without adding a tick.
    pub fn request(&self) -> SimResult {
        let entry = self
            .entry
            .upgrade()
            .ok_or_else(|| SimError("quiescence callback registration is no longer live".into()))?;
        if !entry.committed.get() {
            return Err(SimError(format!(
                "quiescence callback {} is staged, not committed",
                entry.id
            )));
        }
        let pending = entry
            .pending
            .upgrade()
            .ok_or_else(|| SimError("quiescence callback engine is no longer live".into()))?;
        pending.borrow_mut().insert(entry.id);
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct CallbackRegistry {
    entries: RefCell<BTreeMap<u64, Rc<Entry>>>,
    staged: RefCell<Option<BTreeMap<u64, Rc<Entry>>>>,
    pending: Rc<RefCell<BTreeSet<u64>>>,
}

impl CallbackRegistry {
    pub(crate) fn next_id(&self) -> Result<u64, SimError> {
        let entries = self.entries.borrow();
        let staged = self.staged.borrow();
        let mut id = 0u64;
        while entries.contains_key(&id) || staged.as_ref().is_some_and(|s| s.contains_key(&id)) {
            id = id
                .checked_add(1)
                .ok_or_else(|| SimError("Quiescence callback ID space exhausted".into()))?;
        }
        Ok(id)
    }

    pub(crate) fn register(
        &self,
        id: u64,
        callback: Rc<dyn QuiescenceCallback>,
    ) -> Result<QuiescenceHandle, SimError> {
        let mut entries = self.entries.borrow_mut();
        let mut staged = self.staged.borrow_mut();
        if entries.contains_key(&id)
            || staged
                .as_ref()
                .is_some_and(|entries| entries.contains_key(&id))
        {
            return Err(SimError(format!("duplicate quiescence callback ID {id}")));
        }
        let entry = Rc::new(Entry {
            id,
            committed: Cell::new(staged.is_none()),
            callback,
            pending: Rc::downgrade(&self.pending),
        });
        let handle = QuiescenceHandle {
            entry: Rc::downgrade(&entry),
        };
        if let Some(staged) = staged.as_mut() {
            staged.insert(id, entry);
        } else {
            entries.insert(id, entry);
        }
        Ok(handle)
    }

    pub(crate) fn is_staging(&self) -> bool {
        self.staged.borrow().is_some()
    }

    pub(crate) fn begin_staging(&self) -> SimResult {
        let mut staged = self.staged.borrow_mut();
        if staged.is_some() {
            return Err(SimError(
                "nested registration transactions are not supported".into(),
            ));
        }
        *staged = Some(BTreeMap::new());
        Ok(())
    }

    pub(crate) fn commit(&self) {
        let mut staged = self
            .staged
            .borrow_mut()
            .take()
            .expect("active registration transaction");
        for entry in staged.values() {
            entry.committed.set(true);
        }
        self.entries.borrow_mut().append(&mut staged);
    }

    pub(crate) fn rollback(&self) {
        let staged = self.staged.borrow_mut().take();
        drop(staged);
    }

    pub(crate) fn clear(&self) {
        // A callback may own a Spawner/Executor. Break that ownership cycle at
        // engine shutdown. Keep an empty staging marker for an escaped guard.
        self.pending.borrow_mut().clear();
        let entries = std::mem::take(&mut *self.entries.borrow_mut());
        let staged = self.staged.borrow_mut().as_mut().map(std::mem::take);
        drop(entries);
        drop(staged);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.borrow().is_empty()
    }

    pub(crate) fn pending_ids(&self) -> Vec<u64> {
        self.pending.borrow().iter().copied().collect()
    }

    pub(crate) fn run_pending(&self) -> SimResult {
        let snapshot = std::mem::take(&mut *self.pending.borrow_mut());
        for id in snapshot {
            let entry = self.entries.borrow()[&id].clone();
            entry
                .callback
                .on_quiescence()
                .map_err(|error| SimError(format!("quiescence callback {id}: {error}")))?;
        }
        Ok(())
    }

    pub(crate) fn check_drained(&self) -> SimResult {
        let entries: Vec<_> = self.entries.borrow().values().cloned().collect();
        for entry in entries {
            entry.callback.check_drained().map_err(|error| {
                SimError(format!(
                    "stalled simulation: callback {}: {error}",
                    entry.id
                ))
            })?;
        }
        Ok(())
    }
}
