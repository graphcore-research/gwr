// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use gwr_track::entity::{Entity, toplevel};
use gwr_track::tracker::stdout_tracker;
use gwr_track::{Tracker, trace};

use crate::executor::{self, Executor, ExecutorSnapshot, ExecutorStats, Spawner};
use crate::time::clock::Clock;
use crate::types::{Component, SimResult};

/// Use a default clock frequency of 1GHz.
const DEFAULT_CLOCK_MHZ: f64 = 1000.0;

/// Components registered to be spawned when the simulation starts.
///
/// Constructors for active components usually call [`Engine::register`] before
/// returning their `Rc<Self>`. The registry is drained by [`Engine::run`],
/// so all construction and required port connections should be complete
/// before it is called.
pub struct Registry {
    entity: Rc<Entity>,
    components: RefCell<Vec<Component>>,
}

impl Registry {
    fn new(parent: &Rc<Entity>) -> Self {
        Self {
            entity: Rc::new(Entity::new(parent, "registry")),
            components: RefCell::new(Vec::new()),
        }
    }

    pub fn spawn_components(&self, spawner: &Spawner) {
        let mut guard = self.components.borrow_mut();

        trace!(self.entity ; "Spawning {} components", guard.len());

        for component in guard.drain(..) {
            spawner.spawn(async move { component.run().await });
        }
    }

    pub fn register(&self, component: Component) {
        let mut guard = self.components.borrow_mut();
        guard.push(component);
    }
}

/// Single-threaded asynchronous simulation runtime.
///
/// An application normally creates an `Engine`, obtains one or more clocks,
/// constructs components or models with `new_and_register` constructors,
/// connects their ports, and then calls [`run`](Self::run).
/// Connections are intentionally part of setup:
/// a port that is still unconnected when a task tries to use it represents a
/// model-topology error.
///
/// The engine owns the executor, a spawner for additional tasks, the top-level
/// trace [`Entity`], the shared [`Tracker`], and the registry of components
/// that should begin running when simulation starts.
pub struct Engine {
    pub executor: Executor,
    spawner: Spawner,
    toplevel: Rc<Entity>,
    tracker: Tracker,
    registry: Registry,
}

impl Engine {
    /// Create a standalone engine.
    pub fn new(tracker: &Tracker) -> Self {
        let toplevel = toplevel(tracker, "top");
        let (executor, spawner) = executor::new_executor_and_spawner(&toplevel);
        let registry = Registry::new(&toplevel);
        Self {
            executor,
            spawner,
            toplevel,
            tracker: tracker.clone(),
            registry,
        }
    }

    /// Register a component that will be run as the simulation starts.
    ///
    /// Registration should happen during construction, before the engine run
    /// begins. Passive helper objects that do not have independent async
    /// behavior do not need to be registered.
    pub fn register(&self, component: Component) {
        self.registry.register(component);
    }

    pub fn run(&mut self) -> SimResult {
        self.registry.spawn_components(&self.spawner);
        self.executor.run()
    }

    /// Run the simulation while reporting executor activity approximately
    /// after each configured number of future polls.
    ///
    /// Queued task entries in each snapshot do not include futures parked on
    /// clocks, ports, or events. Returns an error when `poll_interval` is zero.
    pub fn run_with_executor_observer(
        &mut self,
        poll_interval: usize,
        observer: impl FnMut(ExecutorSnapshot) -> SimResult,
    ) -> SimResult {
        self.registry.spawn_components(&self.spawner);
        self.executor.run_with_observer(poll_interval, observer)
    }

    #[must_use]
    pub fn spawner(&self) -> Spawner {
        self.spawner.clone()
    }

    pub fn spawn(&self, future: impl Future<Output = SimResult> + 'static) {
        self.spawner.spawn(future);
    }

    pub fn set_randomize_task_order(&self, randomize: bool) {
        self.executor.set_randomize_task_order(randomize);
    }

    pub fn set_task_order_seed(&self, seed: u64) {
        self.executor.set_task_order_seed(seed);
    }

    #[must_use]
    pub fn default_clock(&mut self) -> Clock {
        self.executor.get_clock(DEFAULT_CLOCK_MHZ)
    }

    #[must_use]
    pub fn clock_hz(&mut self, freq_hz: f64) -> Clock {
        self.executor.get_clock(freq_hz / 1_000_000.0)
    }

    #[must_use]
    pub fn clock_khz(&mut self, freq_khz: f64) -> Clock {
        self.executor.get_clock(freq_khz / 1000.0)
    }

    #[must_use]
    pub fn clock_mhz(&mut self, freq_mhz: f64) -> Clock {
        self.executor.get_clock(freq_mhz)
    }

    #[must_use]
    pub fn clock_ghz(&mut self, freq_ghz: f64) -> Clock {
        self.executor.get_clock(freq_ghz * 1000.0)
    }

    #[must_use]
    pub fn time_now_ns(&self) -> f64 {
        self.executor.time_now_ns()
    }

    /// Return cumulative future activity from the most recent run.
    #[must_use]
    pub fn executor_stats(&self) -> ExecutorStats {
        self.executor.stats()
    }

    #[must_use]
    pub fn top(&self) -> &Rc<Entity> {
        &self.toplevel
    }

    #[must_use]
    pub fn tracker(&self) -> Tracker {
        self.tracker.clone()
    }
}

/// Create a default engine that sends [`Track`](gwr_track::Track) events to
/// stdout.
///
/// This is provided to keep documentation examples simple with fewer
/// concepts to have to consider at once.
impl Default for Engine {
    fn default() -> Self {
        let tracker = stdout_tracker(log::Level::Info);
        Self::new(&tracker)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // The tracker can be using a buffered writer and so it needs to be shut
        // down cleanly to ensure that it is flushed properly.
        self.tracker.shutdown();
    }
}
