// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::events::once::Once;
use gwr_engine::quiescence::{QuiescenceCallback, QuiescenceHandle};
use gwr_engine::traits::Runnable;
use gwr_engine::types::{SimError, SimResult};
use gwr_track::tracker::dev_null_tracker;

struct Domain {
    ran: Rc<Cell<usize>>,
    flushed: Rc<Cell<usize>>,
    handle: RefCell<Option<QuiescenceHandle>>,
    fail_drain: bool,
}

#[async_trait(?Send)]
impl Runnable for Domain {
    async fn run(&self) -> SimResult {
        self.ran.set(self.ran.get() + 1);
        self.handle.borrow().as_ref().unwrap().request()
    }
}

impl QuiescenceCallback for Domain {
    fn on_quiescence(&self) -> SimResult {
        self.flushed.set(self.flushed.get() + 1);
        Ok(())
    }

    fn check_drained(&self) -> SimResult {
        if self.fail_drain {
            Err(SimError("discarded observer ran".into()))
        } else {
            Ok(())
        }
    }
}

fn domain(engine: &Engine, id: u64, fail_drain: bool) -> Rc<Domain> {
    let domain = Rc::new(Domain {
        ran: Rc::new(Cell::new(0)),
        flushed: Rc::new(Cell::new(0)),
        handle: RefCell::new(None),
        fail_drain,
    });
    let handle = engine.register_quiescence(id, domain.clone()).unwrap();
    *domain.handle.borrow_mut() = Some(handle);
    engine.register(domain.clone());
    domain
}

#[test]
fn commit_publishes_runnable_and_callback_together() {
    let mut engine = Engine::new(&dev_null_tracker());
    let transaction = engine.registration_transaction().unwrap();
    let domain = domain(&engine, 1, false);
    assert!(domain.handle.borrow().as_ref().unwrap().request().is_err());
    transaction.commit();
    engine.run().unwrap();
    assert_eq!(domain.ran.get(), 1);
    assert_eq!(domain.flushed.get(), 1);
}

#[test]
fn rollback_drops_owned_state_and_invalidates_handles_without_touching_prior_work() {
    let mut engine = Engine::new(&dev_null_tracker());
    let prior = domain(&engine, 1, false);
    prior.handle.borrow().as_ref().unwrap().request().unwrap();
    let transaction = engine.registration_transaction().unwrap();
    let discarded = domain(&engine, 2, true);
    let escaped = discarded.handle.borrow().as_ref().unwrap().clone();
    let weak = Rc::downgrade(&discarded);
    drop(discarded);
    drop(transaction);
    assert!(weak.upgrade().is_none());
    assert!(escaped.request().is_err());
    let replacement = domain(&engine, 2, false);
    assert!(
        escaped.request().is_err(),
        "old handle must not target reused ID"
    );
    engine.run().unwrap();
    assert_eq!(prior.ran.get(), 1);
    assert_eq!(prior.flushed.get(), 1);
    assert_eq!(replacement.ran.get(), 1);
    assert_eq!(replacement.flushed.get(), 1);
}

#[test]
fn nested_transaction_and_execution_during_construction_are_rejected() {
    let mut engine = Engine::new(&dev_null_tracker());
    let prior = domain(&engine, 1, false);
    let transaction = engine.registration_transaction().unwrap();
    assert!(engine.registration_transaction().is_err());
    assert!(engine.run().is_err());
    assert!(engine.run_until(Box::new(Once::default())).is_err());
    let finished = Rc::new(RefCell::new(false));
    assert!(engine.executor.run_until(&finished).is_err());
    assert!(engine.executor.step(&finished).is_err());
    assert_eq!(prior.ran.get(), 0);
    drop(transaction);
    engine.run().unwrap();
    assert_eq!(prior.ran.get(), 1);
}

#[test]
fn constructors_cannot_bypass_staging_with_a_preexisting_spawner() {
    let mut engine = Engine::new(&dev_null_tracker());
    let spawner = engine.spawner();
    let transaction = engine.registration_transaction().unwrap();
    assert!(
        catch_unwind(AssertUnwindSafe(
            || spawner.spawn(async { panic!("escaped task") })
        ))
        .is_err()
    );
    assert!(
        catch_unwind(AssertUnwindSafe(
            || engine.spawn(async { panic!("escaped task") })
        ))
        .is_err()
    );
    drop(transaction);
    engine.run().unwrap();
}

#[test]
fn unwinding_construction_rolls_back_callbacks_and_runnables() {
    let mut engine = Engine::new(&dev_null_tracker());
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _transaction = engine.registration_transaction().unwrap();
            domain(&engine, 3, true);
            panic!("construction failed after callback registration");
        }))
        .is_err()
    );
    engine.run().unwrap();
    let _new_transaction = engine.registration_transaction().unwrap();
}

#[test]
fn engine_drop_releases_self_requesting_callback_and_invalidates_handle() {
    let engine = Engine::new(&dev_null_tracker());
    let domain = domain(&engine, 0, false);
    let weak = Rc::downgrade(&domain);
    let handle = domain.handle.borrow().as_ref().unwrap().clone();
    drop(domain);
    drop(engine);
    assert!(weak.upgrade().is_none());
    assert!(handle.request().is_err());
}

#[test]
fn engine_drop_breaks_callback_ownership_of_a_runtime_spawner() {
    struct OwnsSpawner(gwr_engine::executor::Spawner);
    impl QuiescenceCallback for OwnsSpawner {
        fn on_quiescence(&self) -> SimResult {
            self.0.spawn(async { Ok(()) });
            Ok(())
        }
    }
    let engine = Engine::new(&dev_null_tracker());
    let callback = Rc::new(OwnsSpawner(engine.spawner()));
    let weak = Rc::downgrade(&callback);
    engine.register_quiescence(1, callback).unwrap();
    drop(engine);
    assert!(
        weak.upgrade().is_none(),
        "callback/spawner/registry ownership cycle"
    );
}

#[test]
fn engine_drop_discards_staged_state_even_if_guard_escapes() {
    let engine = Engine::new(&dev_null_tracker());
    let transaction = engine.registration_transaction().unwrap();
    let staged = domain(&engine, 1, true);
    let weak = Rc::downgrade(&staged);
    let handle = staged.handle.borrow().as_ref().unwrap().clone();
    drop(staged);
    drop(engine);
    assert!(weak.upgrade().is_none());
    assert!(handle.request().is_err());
    // The escaped guard can no longer publish any executable state.
    transaction.commit();
    assert!(handle.request().is_err());
}

#[test]
fn dropping_an_escaped_guard_after_engine_destruction_is_safe() {
    let engine = Engine::new(&dev_null_tracker());
    let transaction = engine.registration_transaction().unwrap();
    let staged = domain(&engine, 1, true);
    let handle = staged.handle.borrow().as_ref().unwrap().clone();
    drop(engine);
    drop(transaction);
    assert!(handle.request().is_err());
    assert_eq!(staged.ran.get(), 0);
    assert_eq!(staged.flushed.get(), 0);
}

#[test]
fn duplicate_staged_ids_cannot_replace_committed_or_staged_callbacks() {
    let mut engine = Engine::new(&dev_null_tracker());
    let prior = domain(&engine, 1, false);
    let transaction = engine.registration_transaction().unwrap();
    let staged = domain(&engine, 2, false);
    assert!(engine.register_quiescence(1, staged.clone()).is_err());
    assert!(engine.register_quiescence(2, prior.clone()).is_err());
    transaction.commit();
    engine.run().unwrap();
    assert_eq!(prior.flushed.get(), 1);
    assert_eq!(staged.flushed.get(), 1);
}
#[test]
fn callback_id_suggestions_include_staged_entries_and_rollback() {
    let engine = Engine::new(&dev_null_tracker());
    let first = engine.next_quiescence_id().unwrap();
    let _a = domain(&engine, first, false);
    let transaction = engine.registration_transaction().unwrap();
    let second = engine.next_quiescence_id().unwrap();
    assert_ne!(first, second);
    let _b = domain(&engine, second, false);
    assert_ne!(engine.next_quiescence_id().unwrap(), second);
    drop(transaction);
    assert_eq!(engine.next_quiescence_id().unwrap(), second);
}
