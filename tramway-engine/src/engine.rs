// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use tramway_track::entity::{Entity, toplevel};
use tramway_track::tracker::stdout_tracker;
use tramway_track::{Tracker, trace};

use crate::executor::{self, Executor, Spawner};
use crate::time::clock::Clock;
use crate::types::{Component, Eventable, SimResult};

/// Use a default clock frequency of 1GHz.
const DEFAULT_CLOCK_MHZ: f64 = 1000.0;

pub struct Registry {
    pub entity: Rc<Entity>,
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

    /// Register a component that will be run as the simulation starts
    pub fn register(&self, component: Component) {
        self.registry.register(component);
    }

    pub fn run(&mut self) -> SimResult {
        self.registry.spawn_components(&self.spawner);

        // Pass an atomic bool that will never be set to true
        let finished = Rc::new(RefCell::new(false));
        self.executor.run(&finished)
    }

    pub fn run_until<T: Default + Copy + 'static>(&mut self, event: Eventable<T>) -> SimResult {
        self.registry.spawn_components(&self.spawner);

        // Create an atomic bool that is set to true as soon as the event fires.
        let finished = Rc::new(RefCell::new(false));
        {
            let finished = finished.clone();
            self.spawner.spawn(async move {
                event.listen().await;
                *finished.borrow_mut() = true;
                Ok(())
            });
        }

        self.executor.run(&finished)
    }

    #[must_use]
    pub fn spawner(&self) -> Spawner {
        self.spawner.clone()
    }

    pub fn spawn(&self, future: impl Future<Output = SimResult> + 'static) {
        self.spawner.spawn(future);
    }

    #[must_use]
    pub fn default_clock(&mut self) -> Clock {
        self.executor.get_clock(DEFAULT_CLOCK_MHZ)
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

    #[must_use]
    pub fn top(&self) -> &Rc<Entity> {
        &self.toplevel
    }

    #[must_use]
    pub fn tracker(&self) -> Tracker {
        self.tracker.clone()
    }
}

/// Create a default engine that sends [`Track`](tramway_track::Track) events to
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
        // The tracker can be using a buffered writer and so it needs to be shut down
        // cleanly to ensure that it is flushed properly.
        self.tracker.shutdown();
    }
}
