// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::traits::Event;
use gwr_engine::types::SimResult;
use gwr_track::tracker::dev_null_tracker;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;

struct SelfWakingFuture {
    polls: Rc<Cell<usize>>,
}

impl Future for SelfWakingFuture {
    type Output = SimResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let poll_count = self.polls.get();
        self.polls.set(poll_count + 1);
        assert_eq!(poll_count, 0, "completed task was polled again");

        #[expect(clippy::waker_clone_wake)]
        cx.waker().clone().wake();
        Poll::Ready(Ok(()))
    }
}

#[test]
fn all_spawned() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let top = engine.top();
    let source: Rc<Source<i32>> = Source::new_and_register(&engine, top, "source", None);
    let sink = Sink::new_and_register(&engine, &clock, top, "sink");

    source.connect_port_tx(sink.port_rx()).unwrap();
    run_simulation!(engine);
}

#[test]
fn default_engine_runs() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();

    engine.spawn(async move {
        clock.wait_ticks(1)?.await;
        Ok(())
    });

    run_simulation!(engine);

    assert_eq!(engine.time_now_ns(), 1.0);
}

#[test]
fn task_error_ends_the_current_run() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let later_task_ran = Rc::new(Cell::new(false));

    engine.spawn(async move {
        clock.wait_ticks(3)?.await;
        gwr_engine::sim_error!("simulation stopped")
    });

    let clock = engine.default_clock();
    let later_task_ran_in_future = later_task_ran.clone();
    engine.spawn(async move {
        clock.wait_ticks(10)?.await;
        later_task_ran_in_future.set(true);
        Ok(())
    });

    assert_eq!(engine.run().unwrap_err().0, "simulation stopped");
    assert_eq!(engine.time_now_ns(), 3.0);
    assert!(!later_task_ran.get());
}

#[test]
fn clock_khz_sets_clock_frequency() {
    let mut engine = start_test(file!());
    let clock = engine.clock_khz(1.0);

    engine.spawn(async move {
        clock.wait_ticks(1)?.await;
        Ok(())
    });

    run_simulation!(engine);

    assert_eq!(engine.time_now_ns(), 1_000_000.0);
}

#[test]
fn clock_ghz_sets_clock_frequency() {
    let mut engine = start_test(file!());
    let clock = engine.clock_ghz(2.0);

    engine.spawn(async move {
        clock.wait_ticks(1)?.await;
        Ok(())
    });

    run_simulation!(engine);

    assert_eq!(engine.time_now_ns(), 0.5);
}

#[test]
fn spawner_can_spawn_tasks() {
    let mut engine = start_test(file!());
    let spawner = engine.spawner();
    let ran = Rc::new(Cell::new(false));

    {
        let ran = ran.clone();
        spawner.spawn(async move {
            ran.set(true);
            Ok(())
        });
    }

    run_simulation!(engine);

    assert!(ran.get());
}

#[test]
fn tracker_returns_shared_tracker() {
    let tracker = dev_null_tracker();
    let engine = Engine::new(&tracker);

    assert!(Rc::ptr_eq(&tracker, &engine.tracker()));
}

#[test]
fn self_wake_does_not_repoll_completed_task() {
    // It is essential that the engine does not poll a completed future. The
    // easiest way to ensure the duplicate poll doesn't happen is to create a
    // poll function that wakes itself.
    let mut engine = start_test(file!());
    let polls = Rc::new(Cell::new(0));

    engine.spawn(SelfWakingFuture {
        polls: polls.clone(),
    });

    run_simulation!(engine);
    assert_eq!(polls.get(), 1);
}

#[test]
fn randomized_task_order_uses_seeded_shuffle() {
    const TASKS: usize = 16;
    const SEED: u64 = 1234;

    let mut engine = start_test(file!());
    let order = Rc::new(RefCell::new(Vec::new()));

    engine.set_task_order_seed(SEED);
    engine.set_randomize_task_order(true);

    for task_id in 0..TASKS {
        let order = order.clone();
        engine.spawn(async move {
            order.borrow_mut().push(task_id);
            Ok(())
        });
    }

    run_simulation!(engine);

    let mut expected = (0..TASKS).collect::<Vec<_>>();
    let mut rng = StdRng::seed_from_u64(SEED);
    expected.shuffle(&mut rng);

    assert_eq!(*order.borrow(), expected);
    assert_ne!(*order.borrow(), (0..TASKS).collect::<Vec<_>>());
}

#[derive(Default)]
struct ComponentWithTeardown {
    teardown_count: Cell<u32>,
}

impl gwr_engine::traits::Runnable for ComponentWithTeardown {
    fn teardown(&self) {
        println!("SomethingComponent::teardown {}", self.teardown_count.get());
        self.teardown_count.set(
            self.teardown_count
                .get()
                .checked_add(1)
                .expect("Teardown count overflow"),
        );
    }
}

#[test]
fn dropping_engine_tears_down_and_drops_components() {
    let mut engine = start_test(file!());

    let component = Rc::new(ComponentWithTeardown::default());
    let weak_ref = Rc::downgrade(&component);

    engine.register(component.clone());

    run_simulation!(engine);

    drop(engine);

    // Confirm the engine tore down the component before it was dropped
    assert_eq!(component.teardown_count.get(), 1);

    // Confirm the engine has left no strong component references behind
    drop(component);
    assert_eq!(weak_ref.strong_count(), 0);
}

#[test]
fn dropping_engine_without_running_tears_down_and_drops_components() {
    let engine = start_test(file!());

    let component = Rc::new(ComponentWithTeardown::default());
    let weak_ref = Rc::downgrade(&component);

    engine.register(component.clone());

    // Do nothing - the component shouldn't be torn down before it's been run
    assert_eq!(component.teardown_count.get(), 0);

    drop(engine);

    // Confirm the engine tore down the component before it was dropped
    assert_eq!(component.teardown_count.get(), 1);

    // Confirm the engine has left no strong component references behind
    drop(component);
    assert_eq!(weak_ref.strong_count(), 0);
}

#[test]
fn dropping_engine_drops_completed_tasks() {
    let mut engine = start_test(file!());

    let value = Rc::new(Cell::new(0));
    let weak_ref = Rc::downgrade(&value);

    {
        let value = value.clone();
        engine.spawn(async move {
            value.set(value.get() + 1);
            Ok(())
        });
    }

    run_simulation!(engine);

    // Confirm that the task has been run
    assert_eq!(value.get(), 1);

    // Confirm that dropping the engine and our reference to the test value
    // drops all references to the test value
    drop(value);
    drop(engine);
    assert_eq!(weak_ref.strong_count(), 0);
}

#[test]
fn dropping_engine_without_running_drops_tasks() {
    let engine = start_test(file!());

    let value = Rc::new(Cell::new(0));
    let weak_ref = Rc::downgrade(&value);

    {
        let value = value.clone();
        engine.spawn(async move {
            // Signal that the task has been run
            value.set(value.get() + 1);
            Ok(())
        });
    }

    // Confirm that the task hasn't been started
    assert_eq!(value.get(), 0);

    // Confirm that dropping the engine and our reference to the test value
    // drops all references to the test value
    drop(value);
    drop(engine);
    assert_eq!(weak_ref.strong_count(), 0);
}

#[test]
fn dropping_engine_drops_waiting_tasks() {
    let mut engine = start_test(file!());

    let value = Rc::new(Cell::new(0));
    let weak_ref = Rc::downgrade(&value);

    {
        let value = value.clone();
        let clock = engine.default_clock();
        engine.spawn(async move {
            // Signal that the task has been run
            value.set(value.get() + 1);

            // Wait until the sim exits, potentially leaving a waker scheduled
            // for this task
            clock.wait_ticks_or_exit(100)?.await;

            // Signal that the task completed
            value.set(value.get() + 1);
            Ok(())
        });
    }

    run_simulation!(engine);

    // Confirm that the sim started the task, and then finished without waiting
    // for it to complete
    assert_eq!(engine.time_now_ns(), 0.0);
    assert_eq!(value.get(), 1);

    // Confirm that dropping the engine and our reference to the test value
    // drops all references to the test value
    drop(value);
    drop(engine);
    assert_eq!(weak_ref.strong_count(), 0);
}

#[test]
fn dropping_engine_drops_waiting_tasks_with_spawners() {
    let mut engine = start_test(file!());

    let value = Rc::new(Cell::new(0));
    let weak_ref = Rc::downgrade(&value);

    {
        let value = value.clone();
        let clock = engine.default_clock();
        let spawner = engine.spawner();
        engine.spawn(async move {
            // Signal that the task has been run
            value.set(value.get() + 1);

            // Do something with the spawner, so it's captured
            spawner.spawn(async move { Ok(()) });

            // Wait until the sim exits, potentially leaving a waker scheduled
            // for this task
            clock.wait_ticks_or_exit(100)?.await;

            // Signal that the task completed
            value.set(value.get() + 1);
            Ok(())
        });
    }

    run_simulation!(engine);

    // Confirm that the sim started the task, and then finished without waiting
    // for it to complete
    assert_eq!(engine.time_now_ns(), 0.0);
    assert_eq!(value.get(), 1);

    // Confirm that dropping the engine and our reference to the test value
    // drops all references to the test value
    drop(value);
    drop(engine);
    assert_eq!(weak_ref.strong_count(), 0);
}

#[test]
fn dropping_engine_after_error_drops_tasks() {
    let mut engine = start_test(file!());

    let start_count = Rc::new(Cell::new(0));
    let finish_count = Rc::new(Cell::new(0));
    let weak_start_count_ref = Rc::downgrade(&start_count);
    let weak_finish_count_ref = Rc::downgrade(&finish_count);

    for _ in 0..5 {
        let start_count = start_count.clone();
        let finish_count = finish_count.clone();
        let clock = engine.default_clock();
        engine.spawn(async move {
            start_count.set(start_count.get() + 1);
            clock.wait_ticks(100)?.await;
            finish_count.set(finish_count.get() + 1);
            Ok(())
        });
    }

    {
        let start_count = start_count.clone();
        let clock = engine.default_clock();
        engine.spawn(async move {
            start_count.set(start_count.get() + 1);
            clock.wait_ticks(5)?.await;
            gwr_engine::sim_error!("some error")
        });
    }

    run_simulation!(engine, "some error");

    // Confirm all tasks started, but done finished
    assert_eq!(start_count.get(), 6);
    assert_eq!(finish_count.get(), 0);

    // Confirm that dropping the engine and our reference to the counters
    // drops all references to the counters
    drop(start_count);
    drop(finish_count);
    drop(engine);
    assert_eq!(weak_start_count_ref.strong_count(), 0);
    assert_eq!(weak_finish_count_ref.strong_count(), 0);
}

#[test]
fn dropping_engine_tolerates_wakes_from_cancelled_tasks() {
    struct NotifyOnDrop(Repeated<()>);

    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            self.0.notify();
        }
    }

    let mut engine = start_test(file!());

    let event = Repeated::new(());
    let value = Rc::new(Cell::new(0));
    let weak_ref = Rc::downgrade(&value);

    for _ in 0..2 {
        let event = event.clone();
        let value = value.clone();

        engine.spawn(async move {
            // Signal that the task has been run
            value.set(value.get() + 1);

            // Set up an event notifcation when this task completes or is
            // dropped
            let _notify_on_drop = NotifyOnDrop(event.clone());

            // Wait for that event - this should block the task until it or the
            // other task is dropped
            event.listen().await;

            // Signal that the task completed
            value.set(value.get() + 1);
            Ok(())
        });
    }

    run_simulation!(engine);

    // Confirm both tasks started
    assert_eq!(value.get(), 2);

    // Dropping a task should drop its NotifyOnDrop instance, which should
    // trigger the event, which would normally wake the other task (if the
    // engine wasn't being shut down)

    // Drop the engine and confirm neither task completed
    drop(engine);
    assert_eq!(value.get(), 2);

    // Confirm the engine has left no strong references behind
    drop(value);
    assert_eq!(weak_ref.strong_count(), 0);
}
