// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use crate::Id;
use crate::tracker::aka::AlternativeNames;
use crate::tracker::{EntityManager, Track, Tracker};

/// Container for multiple [`Tracker`]s
pub struct MultiTracker {
    entity_manager: EntityManager,
    trackers: Vec<Tracker>,
}

impl MultiTracker {
    /// Basic constructor
    /// Add a new tracker
    pub fn add_tracker(&mut self, tracker: Tracker) {
        self.trackers.push(tracker);
    }
}

impl Default for MultiTracker {
    fn default() -> Self {
        Self {
            // Create a local entity_manager that will just be used for handling IDs
            entity_manager: EntityManager::new(log::Level::Error),
            trackers: Vec::new(),
        }
    }
}

impl Track for MultiTracker {
    fn unique_id(&self) -> Id {
        self.entity_manager.unique_id()
    }

    fn is_entity_enabled(&self, id: Id, level: log::Level) -> bool {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(id, level) {
                return true;
            }
        }
        false
    }

    fn monitoring_window_size_for(&self, id: Id) -> Option<u64> {
        for tracker in &self.trackers {
            if let Some(window_size_ticks) = tracker.monitoring_window_size_for(id) {
                return Some(window_size_ticks);
            }
        }
        None
    }

    fn add_entity(&self, id: Id, entity_name: &str, alternative_names: AlternativeNames) {
        for tracker in &self.trackers {
            tracker.add_entity(id, entity_name, alternative_names);
        }
    }

    fn enter(&self, id: Id, object: Id) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(id, log::Level::Trace) {
                tracker.enter(id, object);
            }
        }
    }

    fn exit(&self, id: Id, object: Id) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(id, log::Level::Trace) {
                tracker.exit(id, object);
            }
        }
    }

    fn value(&self, id: Id, value: f64) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(id, log::Level::Trace) {
                tracker.value(id, value);
            }
        }
    }

    fn create(&self, created_by: Id, id: Id, num_bytes: usize, req_type: i8, name: &str) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(id, log::Level::Trace) {
                tracker.create(created_by, id, num_bytes, req_type, name);
            }
        }
    }

    fn destroy(&self, destroyed_by: Id, id: Id) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(id, log::Level::Trace) {
                tracker.destroy(destroyed_by, id);
            }
        }
    }

    fn connect(&self, connect_from: Id, connect_to: Id) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(connect_from, log::Level::Trace) {
                tracker.connect(connect_from, connect_to);
            }
        }
    }

    fn log(&self, id: Id, level: log::Level, msg: std::fmt::Arguments) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(id, level) {
                tracker.log(id, level, msg);
            }
        }
    }

    fn time(&self, set_by: Id, time_ns: f64) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(set_by, log::Level::Trace) {
                tracker.time(set_by, time_ns);
            }
        }
    }

    fn shutdown(&self) {
        for tracker in &self.trackers {
            tracker.shutdown();
        }
    }
}
