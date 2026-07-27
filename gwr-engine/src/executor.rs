// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use gwr_track::entity::Entity;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;

use crate::time::clock::Clock;
use crate::time::simtime::SimTime;
use crate::types::SimResult;

type TaskId = usize;

unsafe fn drop_task(data: *const ()) {
    unsafe {
        drop(Arc::from_raw(data as *const TaskWake));
    }
}

static VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_raw_waker, wake_task, wake_by_ref, drop_task);

fn task_raw_waker(wake: Arc<TaskWake>) -> RawWaker {
    let ptr = Arc::into_raw(wake) as *const ();
    RawWaker::new(ptr, &VTABLE)
}

fn waker_for_task(task: &Rc<Task>) -> Waker {
    let wake = Arc::new(TaskWake {
        task_id: task.id,
        external_wake: ExternalWakeHandle {
            state: task.executor_state.external.clone(),
        },
    });

    unsafe { Waker::from_raw(task_raw_waker(wake)) }
}

unsafe fn clone_raw_waker(data: *const ()) -> RawWaker {
    unsafe {
        // Raw wakers store an Arc<TaskWake>. The input pointer is borrowed here,
        // so reconstruct it only long enough to clone it.
        let wake = Arc::from_raw(data as *const TaskWake);
        let clone = wake.clone();
        mem::forget(wake);
        let ptr = Arc::into_raw(clone) as *const ();
        RawWaker::new(ptr, &VTABLE)
    }
}

unsafe fn wake_task(data: *const ()) {
    unsafe {
        // We can safely take ownership of the Arc here, because the waker is being
        // consumed.
        let wake = Arc::from_raw(data as *const TaskWake);
        wake.wake();
        drop(wake);
    }
}

unsafe fn wake_by_ref(data: *const ()) {
    unsafe {
        let wake = Arc::from_raw(data as *const TaskWake);
        wake.wake();
        mem::forget(wake);
    }
}

struct TaskWake {
    task_id: TaskId,
    external_wake: ExternalWakeHandle,
}

impl TaskWake {
    fn wake(&self) {
        self.external_wake.wake_task(self.task_id);
    }
}

struct Task {
    id: TaskId,
    future: RefCell<Option<Pin<Box<dyn Future<Output = SimResult>>>>>,
    executor_state: Rc<ExecutorState>,
}

impl Task {
    pub fn new(
        id: TaskId,
        future: impl Future<Output = SimResult> + 'static,
        executor_state: Rc<ExecutorState>,
    ) -> Task {
        Task {
            id,
            future: RefCell::new(Some(Box::pin(future))),
            executor_state,
        }
    }

    fn poll(&self, context: &mut Context) -> Poll<SimResult> {
        let mut future_slot = self.future.borrow_mut();
        let Some(future) = future_slot.as_mut() else {
            return Poll::Ready(Ok(()));
        };

        let poll_result = future.as_mut().poll(context);
        if poll_result.is_ready() {
            future_slot.take();
        }

        poll_result
    }
}

struct ExecutorState {
    task_queue: RefCell<Vec<Rc<Task>>>,
    new_tasks: RefCell<Vec<Rc<Task>>>,
    next_task_id: Cell<TaskId>,
    time: RefCell<SimTime>,
    randomize_task_order: Cell<bool>,
    task_order_rng: RefCell<StdRng>,
    external: Arc<ExternalState>,
    tasks: RefCell<Vec<Option<Rc<Task>>>>,
}

impl ExecutorState {
    pub fn new(top: &Rc<Entity>) -> Self {
        Self {
            task_queue: RefCell::new(Vec::new()),
            new_tasks: RefCell::new(Vec::new()),
            next_task_id: Cell::new(0),
            time: RefCell::new(SimTime::new(top)),
            randomize_task_order: Cell::new(false),
            task_order_rng: RefCell::new(StdRng::seed_from_u64(rand::random())),
            external: Arc::new(ExternalState {
                data: Mutex::new(ExternalStateData {
                    waits: 0,
                    time_blocking_waits: 0,
                    wakes: Vec::new(),
                }),
                changed: Condvar::new(),
            }),
            tasks: RefCell::new(Vec::new()),
        }
    }
}

struct ExternalStateData {
    waits: usize,
    time_blocking_waits: usize,
    wakes: Vec<TaskId>,
}

struct ExternalState {
    data: Mutex<ExternalStateData>,
    changed: Condvar,
}

#[derive(Clone)]
struct ExternalWakeHandle {
    state: Arc<ExternalState>,
}

impl ExternalWakeHandle {
    fn wake_task(&self, task_id: TaskId) {
        {
            let mut external_wakes = self.state.data.lock().unwrap();
            external_wakes.wakes.push(task_id);
        }
        self.state.changed.notify_all();
    }
}

/// Handle used to tell the executor that a task is waiting for external
/// progress.
#[derive(Clone)]
pub struct ExternalWaitHandle {
    state: Arc<ExternalState>,
}

/// Guard that keeps the executor alive while a task waits for external
/// progress.
pub struct ExternalWaitGuard {
    state: Arc<ExternalState>,
    blocks_time: bool,
}

impl ExternalWaitHandle {
    /// Register that a task is waiting for progress from outside this executor.
    #[must_use]
    pub fn begin_wait(&self) -> ExternalWaitGuard {
        let mut state = self.state.data.lock().unwrap();
        state.waits += 1;
        ExternalWaitGuard {
            state: self.state.clone(),
            blocks_time: false,
        }
    }

    /// Register that a task is waiting for external progress and simulated time
    /// must not advance until that progress happens.
    #[must_use]
    pub fn begin_time_blocking_wait(&self) -> ExternalWaitGuard {
        let mut state = self.state.data.lock().unwrap();
        state.waits += 1;
        state.time_blocking_waits += 1;
        ExternalWaitGuard {
            state: self.state.clone(),
            blocks_time: true,
        }
    }
}

impl Drop for ExternalWaitGuard {
    fn drop(&mut self) {
        {
            let mut state = self.state.data.lock().unwrap();
            assert!(
                state.waits > 0,
                "ExternalWaitGuard dropped without a matching begin_wait"
            );
            state.waits -= 1;
            if self.blocks_time {
                assert!(
                    state.time_blocking_waits > 0,
                    "ExternalWaitGuard dropped without a matching begin_time_blocking_wait"
                );
                state.time_blocking_waits -= 1;
            }
        }
        self.state.changed.notify_all();
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
    pub fn run(&self, finished: &Rc<RefCell<bool>>) -> SimResult {
        loop {
            self.step(finished)?;
            if *finished.borrow() {
                break;
            }

            self.queue_external_wakes();

            if self.state.new_tasks.borrow().is_empty() {
                if self.can_exit_now() {
                    break;
                }

                if self.has_time_blocking_external_waits() {
                    self.wait_for_time_blocking_external_progress();
                } else if let Some(wakers) = self.state.time.borrow_mut().advance_time() {
                    // No events left, advance time
                    for task_waker in wakers.into_iter() {
                        task_waker.waker.wake();
                    }
                } else if self.has_external_waits() {
                    self.wait_for_external_progress();
                } else {
                    break;
                }
            }
        }
        Ok(())
    }

    fn wait_for_external_progress(&self) {
        let mut external_waits = self.state.external.data.lock().unwrap();
        while external_waits.waits > 0 && external_waits.wakes.is_empty() {
            external_waits = self.state.external.changed.wait(external_waits).unwrap();
        }
    }

    fn wait_for_time_blocking_external_progress(&self) {
        let mut external_waits = self.state.external.data.lock().unwrap();
        while external_waits.time_blocking_waits > 0 && external_waits.wakes.is_empty() {
            external_waits = self.state.external.changed.wait(external_waits).unwrap();
        }
    }

    fn take_external_wakes(&self) -> Vec<TaskId> {
        let mut external_wakes = self.state.external.data.lock().unwrap();
        std::mem::take(&mut external_wakes.wakes)
    }

    fn queue_external_wakes(&self) {
        let tasks = self.state.tasks.borrow();
        let new_tasks = &mut self.state.new_tasks.borrow_mut();

        for task_id in self.take_external_wakes() {
            if let Some(Some(task)) = tasks.get(task_id) {
                new_tasks.push(task.clone());
            }
        }
    }

    fn can_exit_now(&self) -> bool {
        self.state.time.borrow().can_exit() && !self.has_external_waits()
    }

    fn has_external_waits(&self) -> bool {
        let external_waits = self.state.external.data.lock().unwrap();
        external_waits.waits > 0
    }

    fn has_time_blocking_external_waits(&self) -> bool {
        let external_waits = self.state.external.data.lock().unwrap();
        external_waits.time_blocking_waits > 0
    }

    pub fn step(&self, finished: &Rc<RefCell<bool>>) -> SimResult {
        self.queue_external_wakes();

        // Append new tasks created since the last step into the task queue
        let mut task_queue = self.state.task_queue.borrow_mut();
        task_queue.append(&mut self.state.new_tasks.borrow_mut());
        if self.state.randomize_task_order.get() {
            task_queue.shuffle(&mut *self.state.task_order_rng.borrow_mut());
        }

        // Loop over all tasks, polling them. If a task is not ready, add it to the
        // pending tasks.
        for task in task_queue.drain(..) {
            if *finished.borrow() {
                break;
            }

            // Dummy waker and context (not used as we poll all tasks)
            let waker = waker_for_task(&task);
            let mut context = Context::from_waker(&waker);

            match task.poll(&mut context) {
                Poll::Ready(Err(e)) => {
                    // Error - return early
                    self.state.tasks.borrow_mut()[task.id] = None;
                    return Err(e);
                }
                Poll::Ready(Ok(())) => {
                    // Otherwise, drop task as it is complete
                    self.state.tasks.borrow_mut()[task.id] = None;
                }
                Poll::Pending => {
                    // Task will have parked itself waiting somewhere
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn external_wait_handle(&self) -> ExternalWaitHandle {
        ExternalWaitHandle {
            state: self.state.external.clone(),
        }
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
        let id = self.state.next_task_id.get();
        self.state.next_task_id.set(id + 1);

        let task = Rc::new(Task::new(id, future, self.state.clone()));
        self.state.tasks.borrow_mut().push(Some(task.clone()));

        self.state.new_tasks.borrow_mut().push(task);
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
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use futures::task::noop_waker;
    use gwr_track::entity::toplevel;
    use gwr_track::tracker::dev_null_tracker;

    use super::*;
    use crate::time::clock::TaskWaker;

    struct StoresWakerThenCompletes {
        polls: Rc<Cell<usize>>,
        stored_waker: Arc<Mutex<Option<Waker>>>,
    }

    impl Future for StoresWakerThenCompletes {
        type Output = SimResult;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let polls = self.polls.get() + 1;
            self.polls.set(polls);

            if polls == 1 {
                *self.stored_waker.lock().unwrap() = Some(cx.waker().clone());
                Poll::Pending
            } else {
                Poll::Ready(Ok(()))
            }
        }
    }

    struct WaitsForExternalProgress {
        wait_handle: ExternalWaitHandle,
        wait: Arc<Mutex<Option<ExternalWaitGuard>>>,
        blocks_time: bool,
        started: bool,
    }

    impl Future for WaitsForExternalProgress {
        type Output = SimResult;

        fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            if !self.started {
                let wait = if self.blocks_time {
                    self.wait_handle.begin_time_blocking_wait()
                } else {
                    self.wait_handle.begin_wait()
                };
                *self.wait.lock().unwrap() = Some(wait);
                self.started = true;
            }
            Poll::Pending
        }
    }

    struct PanicIfPolled;

    impl Future for PanicIfPolled {
        type Output = SimResult;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            panic!("finished executor step polled a task");
        }
    }

    #[test]
    fn stored_waker_can_wake_task_from_another_thread() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);

        let polls = Rc::new(Cell::new(0));
        let stored_waker = Arc::new(Mutex::new(None));

        spawner.spawn(StoresWakerThenCompletes {
            polls: polls.clone(),
            stored_waker: stored_waker.clone(),
        });

        let finished = Rc::new(RefCell::new(false));
        executor.step(&finished).unwrap();
        assert_eq!(polls.get(), 1, "Task should have been polled once");

        let waker_option = stored_waker.lock().unwrap().take();
        assert!(
            waker_option.is_some(),
            "Waker should have been stored in the mutex"
        );

        let waker_clone = waker_option.unwrap();
        let thread_handle = std::thread::spawn(move || {
            waker_clone.wake();
        });

        thread_handle.join().unwrap();

        executor.step(&finished).unwrap();
        assert_eq!(polls.get(), 2, "Task should have been polled twice");

        assert!(
            executor.state.tasks.borrow()[0].as_ref().is_none(),
            "Task should be removed after completing"
        );
    }

    #[test]
    fn external_wait_blocks_time_advance() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);
        let clock = executor.get_clock(1000.0);

        let wait = Arc::new(Mutex::new(None));
        let late_task_ran = Arc::new(AtomicBool::new(false));

        spawner.spawn(WaitsForExternalProgress {
            wait_handle: executor.external_wait_handle(),
            wait: wait.clone(),
            blocks_time: true,
            started: false,
        });

        let late_task_clock = clock.clone();
        let late_task_ran_for_task = late_task_ran.clone();
        spawner.spawn(async move {
            late_task_clock.wait_ticks(10).await;
            late_task_ran_for_task.store(true, Ordering::SeqCst);
            Ok(())
        });

        let time_advanced_before_wait_finished = Arc::new(AtomicBool::new(false));
        let time_advanced_for_thread = time_advanced_before_wait_finished.clone();
        let thread_handle = std::thread::spawn(move || {
            while wait.lock().unwrap().is_none() {
                std::thread::yield_now();
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            time_advanced_for_thread.store(late_task_ran.load(Ordering::SeqCst), Ordering::SeqCst);
            drop(wait.lock().unwrap().take());
        });

        executor.run(&Rc::new(RefCell::new(false))).unwrap();
        thread_handle.join().unwrap();
        assert!(!time_advanced_before_wait_finished.load(Ordering::SeqCst));
    }

    #[test]
    fn external_wait_allows_time_advance() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);
        let clock = executor.get_clock(1000.0);

        let wait = Arc::new(Mutex::new(None));
        let late_task_ran = Arc::new(AtomicBool::new(false));

        spawner.spawn(WaitsForExternalProgress {
            wait_handle: executor.external_wait_handle(),
            wait: wait.clone(),
            blocks_time: false,
            started: false,
        });

        let late_task_clock = clock.clone();
        let late_task_ran_for_task = late_task_ran.clone();
        spawner.spawn(async move {
            late_task_clock.wait_ticks(10).await;
            late_task_ran_for_task.store(true, Ordering::SeqCst);
            Ok(())
        });

        let time_advanced_before_wait_finished = Arc::new(AtomicBool::new(false));
        let time_advanced_for_thread = time_advanced_before_wait_finished.clone();
        let thread_handle = std::thread::spawn(move || {
            while wait.lock().unwrap().is_none() {
                std::thread::yield_now();
            }

            for _ in 0..100 {
                if late_task_ran.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            time_advanced_for_thread.store(late_task_ran.load(Ordering::SeqCst), Ordering::SeqCst);
            drop(wait.lock().unwrap().take());
        });

        executor.run(&Rc::new(RefCell::new(false))).unwrap();
        thread_handle.join().unwrap();
        assert!(time_advanced_before_wait_finished.load(Ordering::SeqCst));
    }

    #[test]
    #[should_panic(expected = "finished executor step polled a task")]
    fn panic_if_polled_panics_when_polled() {
        let mut future = Box::pin(PanicIfPolled);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let _ = future.as_mut().poll(&mut cx);
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

        let finished = Rc::new(RefCell::new(false));

        executor.run(&finished).unwrap();
    }

    #[test]
    fn step_stops_polling_when_finished_is_set() {
        let tracker = dev_null_tracker();
        let top = toplevel(&tracker, "top");
        let (executor, spawner) = new_executor_and_spawner(&top);

        spawner.spawn(PanicIfPolled);

        let finished = Rc::new(RefCell::new(true));

        executor.step(&finished).unwrap();
    }
}
