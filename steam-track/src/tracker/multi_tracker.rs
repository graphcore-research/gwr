// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use crate::Tag;
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
            // Create a local entity_manager that will just be used for handling tags
            entity_manager: EntityManager::new(log::Level::Error),
            trackers: Vec::new(),
        }
    }
}

impl Track for MultiTracker {
    fn unique_tag(&self) -> Tag {
        self.entity_manager.unique_tag()
    }

    fn is_entity_enabled(&self, tag: Tag, level: log::Level) -> bool {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(tag, level) {
                return true;
            }
        }
        false
    }

    fn add_entity(&self, tag: Tag, entity_name: &str) {
        for tracker in &self.trackers {
            tracker.add_entity(tag, entity_name);
        }
    }

    fn enter(&self, tag: Tag, object: Tag) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(tag, log::Level::Trace) {
                tracker.enter(tag, object);
            }
        }
    }

    fn exit(&self, tag: Tag, object: Tag) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(tag, log::Level::Trace) {
                tracker.exit(tag, object);
            }
        }
    }

    fn create(&self, created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(tag, log::Level::Trace) {
                tracker.create(created_by, tag, num_bytes, req_type, name);
            }
        }
    }

    fn destroy(&self, destroyed_by: Tag, tag: Tag) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(tag, log::Level::Trace) {
                tracker.destroy(destroyed_by, tag);
            }
        }
    }

    fn connect(&self, connect_from: Tag, connect_to: Tag) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(connect_from, log::Level::Trace) {
                tracker.connect(connect_from, connect_to);
            }
        }
    }

    fn log(&self, tag: Tag, level: log::Level, msg: std::fmt::Arguments) {
        for tracker in &self.trackers {
            if tracker.is_entity_enabled(tag, level) {
                tracker.log(tag, level, msg);
            }
        }
    }

    fn time(&self, set_by: Tag, time_ns: f64) {
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
