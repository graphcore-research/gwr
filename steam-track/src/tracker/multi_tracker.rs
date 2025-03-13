// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::sync::Arc;

use crate::tracker::{EntityManager, Track, Tracker};
use crate::{Tag, TraceState};

/// Container for multiple [`Tracker`]s
pub struct MultiTracker {
    entity_manager: Arc<EntityManager>,
    trackers: Vec<Tracker>,
}

impl MultiTracker {
    /// Basic constructor
    pub fn new(default_trace_enabled: TraceState, default_log_level: log::Level) -> Self {
        Self {
            entity_manager: Arc::new(EntityManager::new(default_trace_enabled, default_log_level)),
            trackers: Vec::new(),
        }
    }

    /// Add a new tracker
    pub fn add_tracker(&mut self, tracker: Tracker) {
        self.trackers.push(tracker);
    }

    /// Getter for the internal [`EntityManager`]
    pub fn get_entity_manager(&self) -> Arc<EntityManager> {
        self.entity_manager.clone()
    }
}

impl Track for MultiTracker {
    fn unique_tag(&self) -> Tag {
        self.entity_manager.unique_tag()
    }

    fn get_entity_enables(&self, entity_name: &str) -> (bool, log::Level) {
        (
            self.entity_manager.trace_enabled_for(entity_name),
            self.entity_manager.log_level_for(entity_name),
        )
    }

    fn enter(&self, tag: Tag, object: Tag) {
        for tracker in &self.trackers {
            tracker.enter(tag, object);
        }
    }

    fn exit(&self, tag: Tag, object: Tag) {
        for tracker in &self.trackers {
            tracker.exit(tag, object);
        }
    }

    fn create(&self, created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        for tracker in &self.trackers {
            tracker.create(created_by, tag, num_bytes, req_type, name);
        }
    }

    fn destroy(&self, destroyed_by: Tag, tag: Tag) {
        for tracker in &self.trackers {
            tracker.destroy(destroyed_by, tag);
        }
    }

    fn log(&self, tag: Tag, level: log::Level, msg: std::fmt::Arguments) {
        for tracker in &self.trackers {
            tracker.log(tag, level, msg);
        }
    }

    fn time(&self, set_by: Tag, time_ns: f64) {
        for tracker in &self.trackers {
            tracker.time(set_by, time_ns);
        }
    }
}
