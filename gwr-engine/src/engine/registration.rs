// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use super::Registry;
use crate::quiescence::CallbackRegistry;

/// Stages construction registrations without changing constructor signatures.
///
/// Dropping without [`commit`](Self::commit) removes staged runnables,
/// callbacks, and callback-owned drain observers. Existing committed
/// registrations remain. Entity IDs and construction trace records are not
/// rolled back.
///
/// While a transaction exists, nested transactions and execution are rejected;
/// spawning runtime futures panics before queuing them. Constructors must use
/// `Engine::register`, not spawn. The guard owns shared state rather than an
/// exclusive engine borrow so existing `&Engine` constructors keep working.
#[must_use = "dropping the transaction rolls back all staged registrations"]
pub struct RegistrationTransaction {
    registry: Rc<Registry>,
    callbacks: Rc<CallbackRegistry>,
    committed: bool,
}

impl RegistrationTransaction {
    pub(super) fn new(registry: Rc<Registry>, callbacks: Rc<CallbackRegistry>) -> Self {
        *registry.staged.borrow_mut() = Some(Vec::new());
        Self {
            registry,
            callbacks,
            committed: false,
        }
    }

    /// Publish components and callback-owned state synchronously. No execution
    /// or callback delivery occurs between the two registry updates.
    pub fn commit(mut self) {
        let mut components = self
            .registry
            .staged
            .borrow_mut()
            .take()
            .expect("active registration transaction");
        self.registry
            .components
            .borrow_mut()
            .append(&mut components);
        self.callbacks.commit();
        self.committed = true;
    }
}

impl Drop for RegistrationTransaction {
    fn drop(&mut self) {
        if !self.committed {
            let components = self.registry.staged.borrow_mut().take();
            drop(components);
            self.callbacks.rollback();
        }
    }
}
