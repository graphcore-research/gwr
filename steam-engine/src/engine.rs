// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Release;

use steam_track::Tracker;
use steam_track::entity::{Entity, toplevel};
use steam_track::tracker::stdout_tracker;

use crate::executor::{self, Executor, Spawner};
use crate::time::clock::Clock;
use crate::types::{Eventable, SimResult};

/// Use a default clock frequency of 1GHz.
const DEFAULT_CLOCK_MHZ: f64 = 1000.0;

pub struct Engine {
    pub executor: Executor,
    pub spawner: Spawner,
    toplevel: Arc<Entity>,
    tracker: Tracker,
}

impl Engine {
    /// Create a standalone engine.
    pub fn new(tracker: &Tracker) -> Self {
        let toplevel = toplevel(tracker, "top");
        let (executor, spawner) = executor::new_executor_and_spawner(&toplevel);
        Self {
            executor,
            spawner,
            toplevel,
            tracker: tracker.clone(),
        }
    }

    pub fn run(&mut self) -> SimResult {
        // Pass an atomic bool that will never be set to true
        let finished = Rc::new(AtomicBool::new(false));
        self.executor.run(finished)
    }

    pub fn run_until<T: Default + Copy + 'static>(&mut self, event: Eventable<T>) -> SimResult {
        // Create an atomic bool that is set to true as soon as the event fires.
        let finished = Rc::new(AtomicBool::new(false));
        {
            let finished = finished.clone();
            self.executor.spawn(async move {
                event.listen().await;
                finished.store(true, Release);
                Ok(())
            });
        }

        self.executor.run(finished)
    }

    pub fn spawn(&self, future: impl Future<Output = SimResult> + 'static) {
        self.executor.spawn(future);
    }

    pub fn default_clock(&mut self) -> Clock {
        self.executor.get_clock(DEFAULT_CLOCK_MHZ)
    }

    pub fn clock_mhz(&mut self, freq_mhz: f64) -> Clock {
        self.executor.get_clock(freq_mhz)
    }

    pub fn clock_ghz(&mut self, freq_ghz: f64) -> Clock {
        self.executor.get_clock(freq_ghz * 1000.0)
    }

    pub fn time_now_ns(&self) -> f64 {
        self.executor.time_now_ns()
    }

    pub fn top(&self) -> &Arc<Entity> {
        &self.toplevel
    }

    pub fn tracker(&self) -> Tracker {
        self.tracker.clone()
    }
}

/// Create a default engine that sends [`Track`](steam_track::Track) events to
/// stdout.
///
/// This is provided to keep documentation examples simple with fewer
/// concepts to have to consider at once.
impl Default for Engine {
    fn default() -> Self {
        let tracker = stdout_tracker();
        Self::new(&tracker)
    }
}
