// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::Duration;
use std::{mem, thread};

use gwr_track::entity::Entity;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;

use crate::time::clock::Clock;
use crate::time::simtime::SimTime;
use crate::types::SimResult;

fn no_op(_: *const ()) {}

unsafe fn drop_task(data: *const ()) {
    unsafe {
        drop(Rc::from_raw(data as *const Task));
    }
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_raw_waker, wake_task, no_op, drop_task);

fn task_raw_waker(task: Rc<Task>) -> RawWaker {
    let ptr = Rc::into_raw(task) as *const ();
    RawWaker::new(ptr, &VTABLE)
}

fn waker_for_task(task: Rc<Task>) -> Waker {
    unsafe { Waker::from_raw(task_raw_waker(task)) }
}

unsafe fn clone_raw_waker(data: *const ()) -> RawWaker {
    unsafe {
        // Tasks are always wrapped in a reference counter to allow them to be
        // shared read-only. The input `data` pointer is borrowed — we must not
        // decrement its refcount, so we mem::forget the reconstructed Rc rather
        // than letting it drop.
        let rc_task = Rc::from_raw(data as *const Task);
        let clone = rc_task.clone();
        mem::forget(rc_task);
        let ptr = Rc::into_raw(clone) as *const ();
        RawWaker::new(ptr, &VTABLE)
    }
}

unsafe fn wake_task(data: *const ()) {
    unsafe {
        // Tasks are always wrapped in a reference counter to allow them to be
        // shared read-only.
        let rc_task = Rc::from_raw(data as *const Task);
        let cloned = rc_task.clone();
        rc_task.executor_state.new_tasks.borrow_mut().push(cloned);
    }
}

struct Task {
    future: RefCell<Option<Pin<Box<dyn Future<Output = SimResult>>>>>,
    executor_state: Rc<ExecutorState>,
}

impl Task {
    pub fn new(
        future: impl Future<Output = SimResult> + 'static,
        executor_state: Rc<ExecutorState>,
    ) -> Task {
        Task {
            future: RefCell::new(Some(Box::pin(future))),
            executor_state,
        }
    }

    fn poll(&self, context: &mut Context) -> Option<Poll<SimResult>> {
        let mut future_slot = self.future.borrow_mut();
        let future = future_slot.as_mut()?;

        let poll_result = future.as_mut().poll(context);
        if poll_result.is_ready() {
            future_slot.take();
        }

        Some(poll_result)
    }
}

/// Cumulative future activity from the most recent executor run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutorStats {
    /// Number of calls made to an async future's `poll` method.
    pub futures_polled: usize,
    /// Number of async futures that returned `Poll::Ready`.
    pub futures_completed: usize,
}

/// Executor activity reported at a configured wall-clock interval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutorSnapshot {
    /// Cumulative future activity so far in this run.
    pub stats: ExecutorStats,
    /// Number of tasks currently active in the executor.
    pub active_task_count: usize,
}

struct ExecutorState {
    task_queue: RefCell<Vec<Rc<Task>>>,
    new_tasks: RefCell<Vec<Rc<Task>>>,
    time: RefCell<SimTime>,
    randomize_task_order: Cell<bool>,
    task_order_rng: RefCell<StdRng>,
    stats: Cell<ExecutorStats>,
}

impl ExecutorState {
    pub fn new(top: &Rc<Entity>) -> Self {
        Self {
            task_queue: RefCell::new(Vec::new()),
            new_tasks: RefCell::new(Vec::new()),
            time: RefCell::new(SimTime::new(top)),
            randomize_task_order: Cell::new(false),
            task_order_rng: RefCell::new(StdRng::seed_from_u64(rand::random())),
            stats: Cell::new(ExecutorStats::default()),
        }
    }
}

/// Single-threaded executor
///
/// This is a thin-wrapper (using [`Rc`]) around the real executor, so that this
/// struct can be cloned and passed around.
///
/// See the [module documentation] for more details.
///
/// [module documentation]: index.html
#[derive(Clone)]
pub struct Executor {
    state: Rc<ExecutorState>,
}

impl Executor {
    /// Run the executor without observing activity.
    pub(crate) fn run(&self) -> SimResult {
        self.run_internal(None, |_| Ok(()))
    }

    /// Run the executor while reporting activity after each wall-clock
    /// interval has passed. Observations occur at completed step boundaries.
    pub(crate) fn run_with_observer(
        &self,
        minimum_interval: Duration,
        observer: impl FnMut(ExecutorSnapshot) -> SimResult,
    ) -> SimResult {
        if minimum_interval.is_zero() {
            return Err(crate::types::SimError(
                "executor observer interval must be positive".to_string(),
            ));
        }

        let refresh_due = Arc::new(AtomicBool::new(false));
        let timer_refresh_due = Arc::clone(&refresh_due);
        let (stop_timer, timer_stop) = mpsc::channel::<()>();
        let timer = thread::spawn(move || {
            // Use the timeout mechanism on the channel to give us time
            // intervals. Will break out when the main thread
            // disconnects the channel.
            while let Err(RecvTimeoutError::Timeout) = timer_stop.recv_timeout(minimum_interval) {
                timer_refresh_due.store(true, Ordering::Relaxed);
            }
        });

        let result = self.run_internal(Some(&refresh_due), observer);
        drop(stop_timer);
        timer
            .join()
            .expect("executor observer timer thread panicked");
        result
    }

    fn run_internal(
        &self,
        refresh_due: Option<&AtomicBool>,
        mut observer: impl FnMut(ExecutorSnapshot) -> SimResult,
    ) -> SimResult {
        let mut stats = ExecutorStats::default();
        let result = (|| {
            loop {
                self.step(&mut stats)?;

                if refresh_due.is_some_and(|due| {
                    due.load(Ordering::Relaxed) && due.swap(false, Ordering::Relaxed)
                }) {
                    observer(ExecutorSnapshot {
                        stats,
                        active_task_count: self.state.new_tasks.borrow().len(),
                    })?;
                }

                if self.state.new_tasks.borrow().is_empty() {
                    if self.state.time.borrow().can_exit() {
                        break;
                    }

                    if let Some(wakers) = self.state.time.borrow_mut().advance_time() {
                        // No events left, advance time
                        for task_waker in wakers.into_iter() {
                            task_waker.waker.wake();
                        }
                    } else {
                        break;
                    }
                }
            }
            Ok(())
        })();
        self.state.stats.set(stats);
        result
    }

    fn step(&self, stats: &mut ExecutorStats) -> SimResult {
        // Append new tasks created since the last step into the task queue
        let mut task_queue = self.state.task_queue.borrow_mut();
        task_queue.append(&mut self.state.new_tasks.borrow_mut());
        if self.state.randomize_task_order.get() {
            task_queue.shuffle(&mut *self.state.task_order_rng.borrow_mut());
        }

        // Loop over all tasks, polling them. If a task is not ready, add it to
        // the pending tasks.
        for task in task_queue.drain(..) {
            // Dummy waker and context (not used as we poll all tasks)
            let waker = waker_for_task(task.clone());
            let mut context = Context::from_waker(&waker);

            if let Some(poll_result) = task.poll(&mut context) {
                stats.futures_polled += 1;
                match poll_result {
                    Poll::Ready(Err(e)) => {
                        stats.futures_completed += 1;
                        return Err(e);
                    }
                    Poll::Ready(Ok(())) => {
                        stats.futures_completed += 1;
                    }
                    Poll::Pending => {}
                }
            }
        }
        Ok(())
    }

    /// Return cumulative future activity from the most recent executor run.
    #[must_use]
    pub fn stats(&self) -> ExecutorStats {
        self.state.stats.get()
    }

    #[must_use]
    pub fn get_clock(&self, freq_mhz: f64) -> Clock {
        self.state.time.borrow_mut().get_clock(freq_mhz)
    }

    #[must_use]
    pub fn time_now_ns(&self) -> f64 {
        self.state.time.borrow().time_now_ns()
    }

    pub fn set_randomize_task_order(&self, randomize: bool) {
        self.state.randomize_task_order.set(randomize);
    }

    pub fn set_task_order_seed(&self, seed: u64) {
        *self.state.task_order_rng.borrow_mut() = StdRng::seed_from_u64(seed);
    }
}

/// `Spawner` spawns new futures into the executor.
#[derive(Clone)]
pub struct Spawner {
    state: Rc<ExecutorState>,
}

impl Spawner {
    pub fn spawn(&self, future: impl Future<Output = SimResult> + 'static) {
        self.state
            .new_tasks
            .borrow_mut()
            .push(Rc::new(Task::new(future, self.state.clone())));
    }
}

#[must_use]
pub fn new_executor_and_spawner(top: &Rc<Entity>) -> (Executor, Spawner) {
    let state = Rc::new(ExecutorState::new(top));
    (
        Executor {
            state: state.clone(),
        },
        Spawner { state },
    )
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use futures::task::noop_waker;
    use gwr_track::entity::toplevel;
    use gwr_track::tracker::dev_null_tracker;

    use super::*;
    use crate::time::clock::TaskWaker;

    struct PendingOnce {
        was_polled: bool,
    }

    impl Future for PendingOnce {
        type Output = SimResult;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.was_polled {
                Poll::Ready(Ok(()))
            } else {
                self.was_polled = true;
                #[expect(clippy::waker_clone_wake)]
                cx.waker().clone().wake();
                Poll::Pending
            }
        }
    }

    #[test]
    fn run_exits_when_time_cannot_advance() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, _spawner) = new_executor_and_spawner(&top);
        let clock = executor.get_clock(1000.0);

        clock
            .shared_state
            .waiting
            .borrow_mut()
            .push(vec![TaskWaker {
                id: 0,
                waker: noop_waker(),
                can_exit: false,
            }]);

        executor.run().unwrap();
    }

    #[test]
    fn executor_observer_reports_once_when_refresh_is_due() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);

        for _ in 0..5 {
            spawner.spawn(async { Ok(()) });
        }

        let mut snapshots = Vec::new();
        let refresh_due = AtomicBool::new(true);
        executor
            .run_internal(Some(&refresh_due), |snapshot| {
                snapshots.push(snapshot);
                Ok(())
            })
            .unwrap();

        assert!(!refresh_due.load(Ordering::Relaxed));

        assert_eq!(
            snapshots,
            vec![ExecutorSnapshot {
                stats: ExecutorStats {
                    futures_polled: 5,
                    futures_completed: 5,
                },
                active_task_count: 0,
            }]
        );
        assert_eq!(
            executor.stats(),
            ExecutorStats {
                futures_polled: 5,
                futures_completed: 5,
            }
        );
    }

    #[test]
    fn executor_observer_skips_steps_without_a_refresh_signal() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);
        spawner.spawn(async { Ok(()) });

        let refresh_due = AtomicBool::new(false);
        let mut snapshots = Vec::new();
        executor
            .run_internal(Some(&refresh_due), |snapshot| {
                snapshots.push(snapshot);
                Ok(())
            })
            .unwrap();

        assert!(snapshots.is_empty());
    }

    #[test]
    fn executor_stats_count_pending_future_polls_separately_from_completion() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);
        spawner.spawn(PendingOnce { was_polled: false });

        executor.run().unwrap();

        assert_eq!(
            executor.stats(),
            ExecutorStats {
                futures_polled: 2,
                futures_completed: 1,
            }
        );
    }

    #[test]
    fn executor_stats_include_futures_that_complete_with_an_error() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);
        spawner.spawn(async { Err(crate::types::SimError("failed".to_string())) });

        let result = executor.run();

        assert!(result.is_err());
        assert_eq!(
            executor.stats(),
            ExecutorStats {
                futures_polled: 1,
                futures_completed: 1,
            }
        );
    }

    #[test]
    fn executor_observer_is_not_called_from_a_step_that_returns_a_task_error() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);
        spawner.spawn(async { Err(crate::types::SimError("task failed".to_string())) });

        let mut snapshots = Vec::new();
        let refresh_due = AtomicBool::new(true);
        let error = executor
            .run_internal(Some(&refresh_due), |snapshot| {
                snapshots.push(snapshot);
                Err(crate::types::SimError("observer failed".to_string()))
            })
            .unwrap_err();

        assert_eq!(error.to_string(), "task failed");
        assert!(snapshots.is_empty());
        assert_eq!(
            executor.stats(),
            ExecutorStats {
                futures_polled: 1,
                futures_completed: 1,
            }
        );
    }

    #[test]
    fn observer_error_occurs_after_the_entire_step_batch() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);
        let completions = Rc::new(Cell::new(0));
        for _ in 0..3 {
            let completions = completions.clone();
            spawner.spawn(async move {
                completions.set(completions.get() + 1);
                Ok(())
            });
        }

        let refresh_due = AtomicBool::new(true);
        let first_error = executor
            .run_internal(Some(&refresh_due), |_| {
                Err(crate::types::SimError("observer failed".to_string()))
            })
            .unwrap_err();
        assert_eq!(first_error.to_string(), "observer failed");
        assert_eq!(completions.get(), 3);
        assert_eq!(
            executor.stats(),
            ExecutorStats {
                futures_polled: 3,
                futures_completed: 3,
            }
        );
    }

    #[test]
    fn executor_observer_rejects_a_zero_wall_clock_interval() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, _spawner) = new_executor_and_spawner(&top);
        let error = executor
            .run_with_observer(Duration::ZERO, |_| Ok(()))
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "executor observer interval must be positive"
        );
    }
}
