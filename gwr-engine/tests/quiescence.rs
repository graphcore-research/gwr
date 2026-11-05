// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::events::once::Once;
use gwr_engine::quiescence::{QuiescenceCallback, QuiescenceHandle};
use gwr_engine::time::clock::phase;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_track::tracker::dev_null_tracker;

struct Callback<F>(F);

impl<F: Fn() -> SimResult> QuiescenceCallback for Callback<F> {
    fn on_quiescence(&self) -> SimResult {
        (self.0)()
    }
}

fn register(engine: &Engine, id: u64, f: impl Fn() -> SimResult + 'static) -> QuiescenceHandle {
    engine
        .register_quiescence(id, Rc::new(Callback(f)))
        .unwrap()
}

#[test]
fn producer_polling_order_does_not_split_a_quiescent_batch() {
    for seed in 0..16 {
        let mut engine = Engine::new(&dev_null_tracker());
        engine.set_task_order_seed(seed);
        engine.set_randomize_task_order(true);
        let clock = engine.default_clock();
        let arrivals = Rc::new(RefCell::new(Vec::new()));
        let batches = Rc::new(RefCell::new(Vec::new()));
        let handle = register(&engine, 4, {
            let arrivals = arrivals.clone();
            let batches = batches.clone();
            move || {
                let mut batch = std::mem::take(&mut *arrivals.borrow_mut());
                batch.sort_unstable();
                batches.borrow_mut().push((clock.tick_now().tick(), batch));
                Ok(())
            }
        });
        for id in [3, 1, 2] {
            let arrivals = arrivals.clone();
            let handle = handle.clone();
            engine.spawn(async move {
                arrivals.borrow_mut().push(id);
                handle.request()
            });
        }
        engine.run().unwrap();
        assert_eq!(*batches.borrow(), [(0, vec![1, 2, 3])]);
    }
}

#[test]
fn callback_snapshot_is_sorted_and_tasks_drain_before_the_next_snapshot() {
    let mut engine = Engine::new(&dev_null_tracker());
    let log = Rc::new(RefCell::new(Vec::new()));
    let producer = Once::default();
    let next = register(&engine, 5, {
        let log = log.clone();
        move || {
            log.borrow_mut().push(5);
            Ok(())
        }
    });
    let second = register(&engine, 20, {
        let log = log.clone();
        move || {
            log.borrow_mut().push(20);
            Ok(())
        }
    });
    let first = register(&engine, 10, {
        let log = log.clone();
        let producer = producer.clone();
        move || {
            log.borrow_mut().push(10);
            producer.notify()?;
            next.request()
        }
    });
    {
        let log = log.clone();
        engine.spawn(async move {
            producer.listen().await;
            log.borrow_mut().push(99);
            Ok(())
        });
    }
    second.request().unwrap();
    first.request().unwrap();
    engine.run().unwrap();
    assert_eq!(*log.borrow(), [10, 20, 99, 5]);
    assert_eq!(engine.time_now_ns(), 0.0);
}

#[test]
fn equal_time_clock_wakes_are_batched_but_later_phases_are_not() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let other = engine.clock_ghz(2.0);
    let arrivals = Rc::new(Cell::new(0));
    let batches = Rc::new(RefCell::new(Vec::new()));
    let handle = register(&engine, 0, {
        let arrivals = arrivals.clone();
        let batches = batches.clone();
        let clock = clock.clone();
        move || {
            batches
                .borrow_mut()
                .push((clock.tick_now(), arrivals.replace(0)));
            Ok(())
        }
    });
    for (producer_clock, ticks, late) in [
        (clock.clone(), 1, false),
        (other, 2, false),
        (clock, 1, true),
    ] {
        let arrivals = arrivals.clone();
        let handle = handle.clone();
        engine.spawn(async move {
            producer_clock.wait_ticks(ticks)?.await;
            if late {
                producer_clock.wait_phase(phase::END).await;
            }
            arrivals.set(arrivals.get() + 1);
            handle.request()
        });
    }
    engine.run().unwrap();
    let batches = batches.borrow();
    assert_eq!(batches.len(), 2);
    assert_eq!(
        (batches[0].0.tick(), batches[0].0.phase(), batches[0].1),
        (1, phase::BEGIN, 2)
    );
    assert_eq!(
        (batches[1].0.tick(), batches[1].0.phase(), batches[1].1),
        (1, phase::END, 1)
    );
}

#[test]
fn duplicate_ids_and_callback_failures_are_reported() {
    let mut engine = Engine::new(&dev_null_tracker());
    let handle = register(&engine, 42, || Err(SimError("invalid allocation".into())));
    assert!(
        engine
            .register_quiescence(42, Rc::new(Callback(|| Ok(()))))
            .is_err()
    );
    handle.request().unwrap();
    let error = engine.run().unwrap_err().to_string();
    assert!(
        error.contains("42") && error.contains("invalid allocation"),
        "{error}"
    );
}

#[test]
fn zero_time_callback_feedback_has_a_diagnostic_limit() {
    let mut engine = Engine::new(&dev_null_tracker());
    let slot = Rc::new(RefCell::new(None::<QuiescenceHandle>));
    let handle = register(&engine, 7, {
        let slot = slot.clone();
        move || slot.borrow().as_ref().unwrap().request()
    });
    *slot.borrow_mut() = Some(handle.clone());
    handle.request().unwrap();
    let error = engine.run().unwrap_err().to_string();
    assert!(
        error.contains("round limit") && error.contains('7') && error.contains("phase"),
        "{error}"
    );
    assert_eq!(engine.time_now_ns(), 0.0);
}

struct DrainObserver {
    pending: Rc<Cell<bool>>,
    checked: Rc<Cell<usize>>,
}

impl QuiescenceCallback for DrainObserver {
    fn on_quiescence(&self) -> SimResult {
        Ok(())
    }

    fn check_drained(&self) -> SimResult {
        self.checked.set(self.checked.get() + 1);
        if self.pending.get() {
            Err(SimError("flow=7 slots=1 waits_for=reply".into()))
        } else {
            Ok(())
        }
    }
}

#[test]
fn full_run_reports_outstanding_domain_state() {
    let mut engine = Engine::new(&dev_null_tracker());
    engine
        .register_quiescence(
            23,
            Rc::new(DrainObserver {
                pending: Rc::new(Cell::new(true)),
                checked: Rc::new(Cell::new(0)),
            }),
        )
        .unwrap();
    let error = engine.run().unwrap_err().to_string();
    assert!(
        error.contains("stalled")
            && error.contains("23")
            && error.contains("flow=7 slots=1 waits_for=reply"),
        "{error}"
    );
}

#[test]
fn drain_check_waits_for_scheduled_service() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let pending = Rc::new(Cell::new(true));
    let checked = Rc::new(Cell::new(0));
    engine
        .register_quiescence(
            23,
            Rc::new(DrainObserver {
                pending: pending.clone(),
                checked: checked.clone(),
            }),
        )
        .unwrap();
    engine.spawn(async move {
        clock.wait_ticks(3)?.await;
        pending.set(false);
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(checked.get(), 1);
    assert_eq!(engine.time_now_ns(), 3.0);
}

#[test]
fn callbacks_run_before_optional_background_exit() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let ran = Rc::new(Cell::new(false));
    let handle = register(&engine, 0, {
        let ran = ran.clone();
        move || {
            ran.set(true);
            Ok(())
        }
    });
    engine.spawn(async move {
        clock.wait_ticks_or_exit(10)?.await;
        Ok(())
    });
    handle.request().unwrap();
    engine.run().unwrap();
    assert!(ran.get());
    assert_eq!(engine.time_now_ns(), 0.0);
}

#[test]
fn run_until_stops_before_another_callback_round_or_drain_check() {
    let mut engine = Engine::new(&dev_null_tracker());
    let stop = Once::default();
    let ran = Rc::new(Cell::new(false));
    let checked = Rc::new(Cell::new(0));
    let next = register(&engine, 2, {
        let ran = ran.clone();
        move || {
            ran.set(true);
            Ok(())
        }
    });
    let first = register(&engine, 1, {
        let stop = stop.clone();
        move || {
            stop.notify()?;
            next.request()
        }
    });
    engine
        .register_quiescence(
            3,
            Rc::new(DrainObserver {
                pending: Rc::new(Cell::new(true)),
                checked: checked.clone(),
            }),
        )
        .unwrap();
    first.request().unwrap();
    engine.run_until(Box::new(stop)).unwrap();
    assert!(!ran.get());
    assert_eq!(checked.get(), 0);
    // The callback queue survives the exit; this does not promise that every
    // kind of suspended detailed task supports engine restart.
    assert!(engine.run().is_err());
    assert!(ran.get());
}

#[test]
fn already_signalled_stop_prevents_queued_callbacks() {
    let mut engine = Engine::new(&dev_null_tracker());
    let stop = Once::default();
    stop.notify().unwrap();
    register(&engine, 1, || panic!("callback after early stop"))
        .request()
        .unwrap();
    engine.run_until(Box::new(stop)).unwrap();
}

#[test]
fn rerequesting_a_member_of_the_current_snapshot_schedules_another_round() {
    let mut engine = Engine::new(&dev_null_tracker());
    let log = Rc::new(RefCell::new(Vec::new()));
    let second = register(&engine, 20, {
        let log = log.clone();
        move || {
            log.borrow_mut().push(20);
            Ok(())
        }
    });
    let first = register(&engine, 10, {
        let log = log.clone();
        let second = second.clone();
        move || {
            log.borrow_mut().push(10);
            second.request()
        }
    });
    first.request().unwrap();
    second.request().unwrap();
    engine.run().unwrap();
    assert_eq!(*log.borrow(), [10, 20, 20]);
}

#[test]
fn round_limit_resets_on_phase_and_time_advancement() {
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let spawner = engine.spawner();
    let count = Rc::new(Cell::new(0));
    let slot = Rc::new(RefCell::new(None::<QuiescenceHandle>));
    let handle = register(&engine, 0, {
        let count = count.clone();
        let slot = slot.clone();
        move || {
            let n = count.get() + 1;
            count.set(n);
            let handle = slot.borrow().as_ref().unwrap().clone();
            if n == 6000 || n == 12000 {
                let clock = clock.clone();
                spawner.spawn(async move {
                    if n == 6000 {
                        clock.wait_phase(1).await;
                    } else {
                        clock.wait_ticks(1)?.await;
                    }
                    handle.request()
                });
            } else if n < 18000 {
                handle.request()?;
            }
            Ok(())
        }
    });
    *slot.borrow_mut() = Some(handle.clone());
    handle.request().unwrap();
    engine.run().unwrap();
    assert_eq!(count.get(), 18000);
    assert_eq!(engine.time_now_ns(), 1.0);
}

#[test]
fn callbacks_cannot_recursively_execute_the_engine() {
    let mut engine = Engine::new(&dev_null_tracker());
    let executor = engine.executor.clone();
    register(&engine, 0, move || {
        let finished = Rc::new(RefCell::new(false));
        assert!(executor.run_until(&finished).is_err());
        assert!(executor.step(&finished).is_err());
        Ok(())
    })
    .request()
    .unwrap();
    engine.run().unwrap();
}
