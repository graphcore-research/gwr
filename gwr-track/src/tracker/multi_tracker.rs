// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use crate::Id;
use crate::entity::Capacity;
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
            tracker.enter(id, object);
        }
    }

    fn exit(&self, id: Id, object: Id) {
        for tracker in &self.trackers {
            tracker.exit(id, object);
        }
    }

    fn value(&self, id: Id, value: f64) {
        for tracker in &self.trackers {
            tracker.value(id, value);
        }
    }

    fn begin_activity(&self, activity: Id, lane: Id, name: &str) {
        for tracker in &self.trackers {
            tracker.begin_activity(activity, lane, name);
        }
    }

    fn add_to_group(&self, activity: Id, group_id: Id) {
        for tracker in &self.trackers {
            tracker.add_to_group(activity, group_id);
        }
    }

    fn remove_from_group(&self, activity: Id, group_id: Id) {
        for tracker in &self.trackers {
            tracker.remove_from_group(activity, group_id);
        }
    }

    fn end_activity(&self, activity: Id) {
        for tracker in &self.trackers {
            tracker.end_activity(activity);
        }
    }

    fn create_entity(&self, created_by: Id, id: Id, name: &str) {
        for tracker in &self.trackers {
            tracker.create_entity(created_by, id, name);
        }
    }

    fn create_monitor(&self, created_by: Id, id: Id, name: &str) {
        for tracker in &self.trackers {
            tracker.create_monitor(created_by, id, name);
        }
    }

    fn create_lane(&self, created_by: Id, id: Id, name: &str) {
        for tracker in &self.trackers {
            tracker.create_lane(created_by, id, name);
        }
    }

    fn create_group(&self, created_by: Id, id: Id, name: &str) {
        for tracker in &self.trackers {
            tracker.create_group(created_by, id, name);
        }
    }

    fn create_object(
        &self,
        created_by: Id,
        id: Id,
        size: usize,
        units: &str,
        req_type: u8,
        details: &str,
    ) {
        for tracker in &self.trackers {
            tracker.create_object(created_by, id, size, units, req_type, details);
        }
    }

    fn capacity(&self, id: Id, capacity: Capacity) {
        for tracker in &self.trackers {
            tracker.capacity(id, capacity.clone());
        }
    }

    fn destroy(&self, destroyed_by: Id, id: Id) {
        for tracker in &self.trackers {
            tracker.destroy(destroyed_by, id);
        }
    }

    fn connect(&self, connect_from: Id, connect_to: Id) {
        for tracker in &self.trackers {
            tracker.connect(connect_from, connect_to);
        }
    }

    fn log(&self, id: Id, level: log::Level, msg: std::fmt::Arguments) {
        for tracker in &self.trackers {
            tracker.log(id, level, msg);
        }
    }

    fn time(&self, set_by: Id, time_ns: f64) {
        for tracker in &self.trackers {
            tracker.time(set_by, time_ns);
        }
    }

    fn shutdown(&self) {
        for tracker in &self.trackers {
            tracker.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use log::Level;

    use super::MultiTracker;
    use crate::Id;
    use crate::test_helpers::{TestTracker, check_and_clear};
    use crate::tracker::{Track, Tracker};

    #[test]
    fn log_events_are_delivered_to_all_sub_trackers_even_if_only_one_enables_the_level() {
        let trace_tracker = Rc::new(TestTracker::new(100, Level::Trace));
        let error_tracker = Rc::new(TestTracker::new(200, Level::Error));

        let mut multi_tracker = MultiTracker::default();
        let trace_tracker_dyn: Tracker = trace_tracker.clone();
        let error_tracker_dyn: Tracker = error_tracker.clone();
        multi_tracker.add_tracker(trace_tracker_dyn);
        multi_tracker.add_tracker(error_tracker_dyn);

        let entity_id = Id(42);
        assert!(multi_tracker.is_entity_enabled(entity_id, Level::Trace));

        multi_tracker.log(entity_id, Level::Trace, format_args!("fanout"));

        check_and_clear(&trace_tracker, &["42:TRACE: fanout"]);
        check_and_clear(&error_tracker, &["42:TRACE: fanout"]);
    }
}
