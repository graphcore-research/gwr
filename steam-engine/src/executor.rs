// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Acquire;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use steam_track::entity::Entity;

use crate::time::clock::Clock;
use crate::time::simtime::SimTime;
use crate::types::SimResult;

fn no_op(_: *const ()) {}

fn task_raw_waker(task: Rc<Task>) -> RawWaker {
    let vtable = &RawWakerVTable::new(clone_raw_waker, wake_task, no_op, no_op);
    let ptr = Rc::into_raw(task) as *const ();
    RawWaker::new(ptr, vtable)
}

fn waker_for_task(task: Rc<Task>) -> Waker {
    unsafe { Waker::from_raw(task_raw_waker(task)) }
}

unsafe fn clone_raw_waker(data: *const ()) -> RawWaker {
    unsafe {
        // Tasks are always wrapped in a reference counter to allow them to be shared
        // read-only.
        let rc_task = Rc::from_raw(data as *const Task);
        let clone = rc_task.clone();
        let vtable = &RawWakerVTable::new(clone_raw_waker, wake_task, no_op, no_op);
        let ptr = Rc::into_raw(clone) as *const ();
        RawWaker::new(ptr, vtable)
    }
}

unsafe fn wake_task(data: *const ()) {
    unsafe {
        // Tasks are always wrapped in a reference counter to allow them to be shared
        // read-only.
        let rc_task = Rc::from_raw(data as *const Task);
        let cloned = rc_task.clone();
        rc_task.executor_state.new_tasks.borrow_mut().push(cloned);
    }
}

struct Task {
    future: RefCell<Pin<Box<dyn Future<Output = SimResult>>>>,
    executor_state: Rc<ExecutorState>,
}

impl Task {
    pub fn new(
        future: impl Future<Output = SimResult> + 'static,
        executor_state: Rc<ExecutorState>,
    ) -> Task {
        Task {
            future: RefCell::new(Box::pin(future)),
            executor_state,
        }
    }

    fn poll(&self, context: &mut Context) -> Poll<SimResult> {
        self.future.borrow_mut().as_mut().poll(context)
    }
}

struct ExecutorState {
    task_queue: RefCell<Vec<Rc<Task>>>,
    new_tasks: RefCell<Vec<Rc<Task>>>,
    time: RefCell<SimTime>,
}

impl ExecutorState {
    pub fn new(top: &Arc<Entity>) -> Self {
        Self {
            task_queue: RefCell::new(Vec::new()),
            new_tasks: RefCell::new(Vec::new()),
            time: RefCell::new(SimTime::new(top)),
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
    pub entity: Arc<Entity>,
    state: Rc<ExecutorState>,
}

impl Executor {
    pub fn spawn(&self, future: impl Future<Output = SimResult> + 'static) {
        self.state
            .new_tasks
            .borrow_mut()
            .push(Rc::new(Task::new(future, self.state.clone())));
    }

    pub fn run(&self, finished: Rc<AtomicBool>) -> SimResult {
        loop {
            self.step(&finished)?;
            if finished.load(Acquire) {
                break;
            }

            if self.state.new_tasks.borrow().is_empty() {
                if let Some(wakers) = self.state.time.borrow_mut().advance_time() {
                    // No events left, advance time
                    for waker in wakers.into_iter() {
                        waker.wake();
                    }
                } else {
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn step(&self, finished: &Rc<AtomicBool>) -> SimResult {
        // Append new tasks created since the last step into the task queue
        let mut task_queue = self.state.task_queue.borrow_mut();
        task_queue.append(&mut self.state.new_tasks.borrow_mut());

        // Loop over all tasks, polling them. If a task is not ready, add it to the
        // pending tasks.
        for task in task_queue.drain(..) {
            if finished.load(Acquire) {
                break;
            }

            // Dummy waker and context (not used as we poll all tasks)
            let waker = waker_for_task(task.clone());
            let mut context = Context::from_waker(&waker);

            match task.poll(&mut context) {
                Poll::Ready(Err(e)) => {
                    // Error - return early
                    return Err(e);
                }
                Poll::Ready(Ok(())) => {
                    // Otherwise, drop task as it is complete
                }
                Poll::Pending => {
                    // Task will have parked itself waiting somewhere
                }
            }
        }
        Ok(())
    }

    pub fn get_clock(&self, freq_mhz: f64) -> Clock {
        self.state.time.borrow_mut().get_clock(freq_mhz)
    }

    pub fn time_now_ns(&self) -> f64 {
        self.state.time.borrow().time_now_ns()
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

pub fn new_executor_and_spawner(top: &Arc<Entity>) -> (Executor, Spawner) {
    let state = Rc::new(ExecutorState::new(top));
    let entity = Arc::new(Entity::new(top, "executor"));
    (
        Executor {
            entity,
            state: state.clone(),
        },
        Spawner { state },
    )
}
