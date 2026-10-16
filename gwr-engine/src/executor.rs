// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Single-threaded task polling and waker ownership.
//!
//! Spawning puts an `Rc<Task>` in `new_tasks`. Each step moves queued handles
//! into a reusable buffer and drains it, polling each handle's future if it
//! has not already completed. Several queued handles can refer to the same
//! task, so a pending future may be polled more than once in a step.
//!
//! Each drained handle is dropped after use. A pending task stays alive only
//! while another strong reference exists, such as a queued handle or a saved
//! waker. There is no separate collection retaining all pending tasks.
//!
//! The borrowed/owned distinction applies to wakers; both refer to the same
//! heap-allocated task. There are two raw-waker representations:
//!
//! - The polling context uses `BORROWED_VTABLE`. Its data points to the local
//!   `Rc<Task>` handle, not directly to the task. `waker_for_task` returns a
//!   `WakerRef` whose lifetime is tied to that handle's borrow, preventing safe
//!   code from retaining this representation after the handle goes away.
//!   `ManuallyDrop` suppresses destruction of the temporary waker. Constructing
//!   this context therefore needs no extra reference-count increment/decrement.
//! - Cloning the context waker clones the borrowed `Rc` and passes that new
//!   owner through `Rc::into_raw`. The result uses `VTABLE`, points directly to
//!   the task, and owns one strong reference. It can outlive the poll. Further
//!   clones each acquire another reference; dropping an owned waker releases
//!   its reference. Consuming it with `wake()` queues a task reference and
//!   releases the reference owned by that waker.
//!
//! The lifetimes enforce the borrowed context's scope, but choosing the correct
//! data pointer and vtable is an unsafe-code invariant: only owned-waker
//! callbacks may reconstruct an `Rc<Task>` with `Rc::from_raw`, and their input
//! must originate from `Rc::into_raw`. A borrowed pointer cannot be treated as
//! an owned reference merely by suppressing its destructor. Both vtables
//! support `wake_by_ref` by cloning one task reference into `new_tasks` while
//! preserving the source waker. The borrowed vtable rejects consuming wake and
//! drop, which safe use of `WakerRef` prevents. Those callbacks panic if
//! called.
//!
//! Completion removes the future from the task, so a later queued wake cannot
//! poll it again. The task is destroyed when its last strong reference is
//! released. These task references are separate from `Rc<ExecutorState>`, which
//! keeps the shared executor state alive.

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

use futures::task::WakerRef;
use gwr_track::entity::Entity;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;

use crate::time::clock::Clock;
use crate::time::simtime::SimTime;
use crate::types::SimResult;

fn no_op(_: *const ()) {
    unreachable!("borrowed executor waker must not be consumed or dropped");
}

unsafe fn drop_task(data: *const ()) {
    // SAFETY: VTABLE supplies an owned task reference created by Rc::into_raw.
    // Dropping the reconstructed Rc releases that reference.
    unsafe {
        drop(Rc::from_raw(data as *const Task));
    }
}

static VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_raw_waker, wake_task, wake_task_by_ref, drop_task);
static BORROWED_VTABLE: RawWakerVTable = RawWakerVTable::new(
    clone_borrowed_waker,
    no_op,
    wake_borrowed_task_by_ref,
    no_op,
);

fn waker_for_task(task: &Rc<Task>) -> WakerRef<'_> {
    // SAFETY: the data points to the Rc handle, which remains borrowed for the
    // lifetime of the returned WakerRef. Cloning produces an owned waker with
    // its own Rc reference and vtable. Wake-by-reference clones into the queue.
    let raw = RawWaker::new(std::ptr::from_ref(task).cast(), &BORROWED_VTABLE);
    let waker = mem::ManuallyDrop::new(unsafe { Waker::from_raw(raw) });
    WakerRef::new_unowned(waker)
}

unsafe fn clone_borrowed_waker(data: *const ()) -> RawWaker {
    // SAFETY: only BORROWED_VTABLE calls this function, and its data points to
    // the live Rc handle borrowed by waker_for_task.
    let task = unsafe { &*data.cast::<Rc<Task>>() };
    RawWaker::new(Rc::into_raw(task.clone()).cast(), &VTABLE)
}

unsafe fn wake_borrowed_task_by_ref(data: *const ()) {
    // SAFETY: BORROWED_VTABLE points to the live Rc handle borrowed by
    // waker_for_task. The queue gets a new reference, leaving that handle
    // intact.
    let task = unsafe { &*data.cast::<Rc<Task>>() };
    task.executor_state
        .new_tasks
        .borrow_mut()
        .push(task.clone());
}

unsafe fn wake_task_by_ref(data: *const ()) {
    // SAFETY: VTABLE points to an owned reference from Rc::into_raw.
    // ManuallyDrop preserves the source waker's ownership of that reference.
    let task = mem::ManuallyDrop::new(unsafe { Rc::from_raw(data.cast::<Task>()) });
    task.executor_state
        .new_tasks
        .borrow_mut()
        .push(Rc::clone(&task));
}

unsafe fn clone_raw_waker(data: *const ()) -> RawWaker {
    // SAFETY: VTABLE is used only for owned pointers from Rc::into_raw.
    // Keep the original reference owned by the source waker.
    unsafe {
        let rc_task = mem::ManuallyDrop::new(Rc::from_raw(data as *const Task));
        let clone = Rc::clone(&rc_task);
        let ptr = Rc::into_raw(clone) as *const ();
        RawWaker::new(ptr, &VTABLE)
    }
}

unsafe fn wake_task(data: *const ()) {
    // SAFETY: VTABLE supplies an owned task reference created by Rc::into_raw.
    // Reconstruct it so the consumed waker's reference is released on return
    // or unwind. The queue receives a separate cloned reference.
    unsafe {
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
    new_tasks: RefCell<Vec<Rc<Task>>>,
    time: RefCell<SimTime>,
    randomize_task_order: Cell<bool>,
    task_order_rng: RefCell<StdRng>,
    stats: Cell<ExecutorStats>,
}

impl ExecutorState {
    pub fn new(top: &Rc<Entity>) -> Self {
        Self {
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

        // Reuse the polling buffer across steps. Start with room for 100 tasks
        // to avoid allocating for typical batches; larger batches grow it.
        let mut task_queue = Vec::with_capacity(100);

        let result = (|| {
            loop {
                self.step(&mut task_queue, &mut stats)?;

                if refresh_due.is_some_and(|due| {
                    due.load(Ordering::Relaxed) && due.swap(false, Ordering::Relaxed)
                }) {
                    observer(ExecutorSnapshot {
                        stats,
                        active_task_count: self.state.new_tasks.borrow().len(),
                    })?;
                }

                if self.state.new_tasks.borrow().is_empty()
                    && !self.state.time.borrow_mut().try_advance()
                {
                    break;
                }
            }
            Ok(())
        })();
        self.state.stats.set(stats);
        result
    }

    // Run a simulation step.
    //
    // The caller passes in task_queue as scratch buffer so its allocation can
    // be reused by each call to step. It is fully drained before step
    // completes unless there is an error.
    fn step(&self, task_queue: &mut Vec<Rc<Task>>, stats: &mut ExecutorStats) -> SimResult {
        // Append new tasks created since the last step into the task queue
        task_queue.append(&mut self.state.new_tasks.borrow_mut());
        if self.state.randomize_task_order.get() {
            task_queue.shuffle(&mut *self.state.task_order_rng.borrow_mut());
        }

        // Poll each queued task once. Pending tasks must arrange to be queued
        // again through a saved waker; they are not retained here.
        for task in task_queue.drain(..) {
            let waker = waker_for_task(&task);
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

    use gwr_track::entity::toplevel;
    use gwr_track::tracker::dev_null_tracker;

    use super::*;

    fn new_executor(top: &Rc<Entity>) -> Executor {
        Executor {
            state: Rc::new(ExecutorState::new(top)),
        }
    }

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
    fn wake_by_ref_queues_task_without_consuming_the_waker() {
        for use_owned_waker in [false, true] {
            let tracker = dev_null_tracker();
            let top = toplevel(&tracker, "top");
            let executor = new_executor(&top);
            let completed = Rc::new(Cell::new(false));
            let completion = completed.clone();
            let task = Rc::new(Task::new(
                async move {
                    completion.set(true);
                    Ok(())
                },
                executor.state.clone(),
            ));
            let borrowed = waker_for_task(&task);
            let owned = use_owned_waker.then(|| Waker::clone(&borrowed));
            let waker = owned.as_ref().unwrap_or(&borrowed);
            let references = Rc::strong_count(&task);

            waker.wake_by_ref();
            waker.wake_by_ref();
            assert_eq!(Rc::strong_count(&task), references + 2);

            let mut stats = ExecutorStats::default();
            executor.step(&mut Vec::new(), &mut stats).unwrap();
            assert!(completed.get());
            assert_eq!(stats.futures_polled, 1);
            assert_eq!(Rc::strong_count(&task), references);
            drop(owned);
            assert_eq!(Rc::strong_count(&task), 1);
        }
    }

    #[test]
    fn poll_context_does_not_hold_an_extra_task_reference() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let executor = new_executor(&top);
        let observed_references = Rc::new(Cell::new(0));
        let task: Rc<Task> = Rc::new_cyclic(|weak| {
            let weak = weak.clone();
            let observed = observed_references.clone();
            Task::new(
                async move {
                    observed.set(weak.strong_count());
                    Ok(())
                },
                executor.state.clone(),
            )
        });
        executor.state.new_tasks.borrow_mut().push(task.clone());

        let mut task_queue = Vec::new();
        executor
            .step(&mut task_queue, &mut ExecutorStats::default())
            .unwrap();

        // The test handle and the executor's task handle
        assert_eq!(observed_references.get(), 2);
        assert_eq!(Rc::strong_count(&task), 1);
    }

    #[test]
    fn cloned_context_waker_keeps_task_alive_after_poll() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let executor = new_executor(&top);
        let saved_waker = Rc::new(RefCell::new(None));
        let saved = saved_waker.clone();
        let mut was_polled = false;
        let future = std::future::poll_fn(move |cx| {
            if was_polled {
                Poll::Ready(Ok(()))
            } else {
                was_polled = true;
                *saved.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
        });
        let task = Rc::new(Task::new(future, executor.state.clone()));
        let weak = Rc::downgrade(&task);
        executor.state.new_tasks.borrow_mut().push(task);
        let mut stats = ExecutorStats::default();
        let mut task_queue = Vec::new();

        executor.step(&mut task_queue, &mut stats).unwrap();
        assert_eq!(weak.strong_count(), 1);
        assert!(executor.state.new_tasks.borrow().is_empty());

        saved_waker.borrow_mut().take().unwrap().wake();
        assert_eq!(executor.state.new_tasks.borrow().len(), 1);
        executor.step(&mut task_queue, &mut stats).unwrap();

        assert_eq!(weak.strong_count(), 0);
        assert_eq!(stats.futures_polled, 2);
        assert_eq!(stats.futures_completed, 1);
    }

    #[test]
    fn owned_waker_clone_survives_borrowed_context_and_original_waker() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let executor = new_executor(&top);
        let task = Rc::new(Task::new(async { Ok(()) }, executor.state.clone()));
        let weak = Rc::downgrade(&task);
        let owned = {
            let borrowed = waker_for_task(&task);
            Waker::clone(&borrowed)
        };
        drop(task);

        let cloned = owned.clone();
        assert_eq!(weak.strong_count(), 2);
        drop(owned);
        assert_eq!(weak.strong_count(), 1);

        cloned.wake();
        assert_eq!(executor.state.new_tasks.borrow().len(), 1);
        let mut task_queue = Vec::new();
        executor
            .step(&mut task_queue, &mut ExecutorStats::default())
            .unwrap();
        assert_eq!(weak.strong_count(), 0);
    }

    #[test]
    fn wake_releases_its_task_reference_when_queue_borrowing_panics() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let executor = new_executor(&top);
        let task = Rc::new(Task::new(async { Ok(()) }, executor.state.clone()));
        let weak = Rc::downgrade(&task);
        let waker = Waker::clone(&waker_for_task(&task));
        drop(task);
        let queue = executor.state.new_tasks.borrow_mut();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| waker.wake()));

        assert!(result.is_err());
        assert!(queue.is_empty());
        assert_eq!(weak.strong_count(), 0);
    }

    #[test]
    fn poll_panic_does_not_consume_the_callers_task_reference() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let executor = new_executor(&top);
        let task = Rc::new(Task::new(
            async { panic!("poll failed") },
            executor.state.clone(),
        ));
        executor.state.new_tasks.borrow_mut().push(task.clone());
        let mut task_queue = Vec::new();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            executor.step(&mut task_queue, &mut ExecutorStats::default())
        }));

        assert!(result.is_err());
        assert!(task_queue.is_empty());
        assert_eq!(Rc::strong_count(&task), 1);
    }

    #[test]
    fn run_exits_when_pending_tasks_have_no_scheduled_events() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);
        spawner.spawn(std::future::pending());

        executor.run().unwrap();
        assert_eq!(executor.stats().futures_polled, 1);
        assert_eq!(executor.stats().futures_completed, 0);
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
        let executor = new_executor(&top);
        let error = executor
            .run_with_observer(Duration::ZERO, |_| Ok(()))
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "executor observer interval must be positive"
        );
    }
}
