// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Define the [`Track`] trait a number of [`Tracker`]s.

/// Include the CapnProto tracker.
pub mod capnp;
/// Include the /dev/null tracker.
pub mod dev_null;
/// Include the in-memory tracker.
pub mod in_memory;
/// Include the text-based tracker.
pub mod text;
/// Include the types required for tracker.
pub mod types;

/// Include the multi-tracker.
pub mod multi_tracker;

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub use capnp::CapnProtoTracker;
pub use dev_null::DevNullTracker;
pub use in_memory::InMemoryTracker;
use regex::Regex;
pub use text::TextTracker;

use crate::{ROOT, Tag};

/// This is the interface that is supported by all [`Tracker`]s.
pub trait Track {
    /// Allocate a new global tag
    fn unique_tag(&self) -> Tag;

    /// Determine whether tracking is enabled, and at what level for an
    /// entity looked up by its tag.
    fn is_entity_enabled(&self, tag: Tag, level: log::Level) -> bool;

    /// Record an entity being created.
    fn add_entity(&self, tag: Tag, entity_name: &str);

    /// Track when an entity with the given tag arrives.
    fn enter(&self, enter_into: Tag, enter_obj: Tag);

    /// Track when an entity with the given tag leaves.
    fn exit(&self, exit_from: Tag, exit_obj: Tag);

    /// Track when an entity with the given tag is created.
    fn create(&self, created_by: Tag, created_obj: Tag, num_bytes: usize, req_type: i8, name: &str);

    /// Track when an entity with the given tag is destroyed.
    fn destroy(&self, destroyed_by: Tag, destroyed_obj: Tag);

    /// Track when an entity is connected to another entity
    fn connect(&self, connect_from: Tag, connect_to: Tag);

    /// Track a log message of the given level.
    fn log(&self, msg_by: Tag, level: log::Level, msg: std::fmt::Arguments);

    /// Advance the time to the time specified in `ns`.
    fn time(&self, set_by: Tag, time_ns: f64);

    /// Perform any pre-exit shutdown/cleanup
    fn shutdown(&self);
}

/// The type of a [`Tracker`] that is shared across entities.
pub type Tracker = Arc<dyn Track + Send + Sync>;

/// Create a [`Tracker`] that prints all track events to `stdout`.
#[must_use]
pub fn stdout_tracker(level: log::Level) -> Tracker {
    let entity_manger = EntityManager::new(level);
    let stdout_writer = Box::new(std::io::BufWriter::new(io::stdout()));
    let tracker: Tracker = Arc::new(TextTracker::new(entity_manger, stdout_writer));
    tracker
}

/// Create a [`Tracker`] that suppresses all track events.
#[must_use]
pub fn dev_null_tracker() -> Tracker {
    let tracer: Tracker = Arc::new(DevNullTracker {});
    tracer
}

/// The [`EntityManager`] is responsible for determining entity log / trace
/// enable states.
///
/// This is shared by the [`Text`](crate::tracker::text) and
/// [`Capnp`](crate::tracker::capnp)-based trackers.
///
/// This manager is also used to allocate unique [`Tag`] values.
pub struct EntityManager {
    /// Level of tracking events to output.
    default_entity_level: log::Level,

    /// List of regular expressions mapping entity names to log levels.
    regex_to_entity_level: Vec<(Regex, log::Level)>,

    /// Used to assign unique tags.
    unique_tag: AtomicU64,

    /// Keep track of the current time.
    current_time: Mutex<f64>,

    /// Keep track of entities that have trace enable/log levels different to
    /// the default
    entity_lookup: Mutex<HashMap<Tag, log::Level>>,
}

impl EntityManager {
    /// Constructor with default [`log::Level`]
    #[must_use]
    pub fn new(default_entity_level: log::Level) -> Self {
        Self {
            default_entity_level,
            regex_to_entity_level: Vec::new(),
            unique_tag: AtomicU64::new(ROOT.0 + 1),
            current_time: Mutex::new(0.0),
            entity_lookup: Mutex::new(HashMap::new()),
        }
    }

    fn unique_tag(&self) -> Tag {
        let tag = self.unique_tag.fetch_add(1, Ordering::SeqCst);
        Tag(tag)
    }

    fn is_enabled(&self, tag: Tag, level: log::Level) -> bool {
        match self.entity_lookup.lock().unwrap().get(&tag) {
            None => level <= self.default_entity_level,
            Some(entity_level) => level <= *entity_level,
        }
    }

    fn add_entity(&self, tag: Tag, entity_name: &str) {
        let entity_level = self.log_level_for(entity_name);
        if entity_level != self.default_entity_level
            && self
                .entity_lookup
                .lock()
                .unwrap()
                .insert(tag, entity_level)
                .is_some()
        {
            panic!("Entity tag {tag} already seen ({entity_name})");
        }
    }

    fn log_level_for(&self, entity_name: &str) -> log::Level {
        for (regex, level) in &self.regex_to_entity_level {
            if regex.is_match(entity_name) {
                return *level;
            }
        }
        self.default_entity_level
    }

    /// Add a filter regular expression to set matching entites to a given
    /// level.
    ///
    /// # Example
    ///
    /// ```rust
    /// use steam_track::tracker::EntityManager;
    /// let mut manager = EntityManager::new(log::Level::Warn);
    /// manager.add_entity_level_filter(".*arb.*", log::Level::Trace);
    /// ```
    pub fn add_entity_level_filter(&mut self, regex_str: &str, level: crate::log::Level) {
        match Regex::new(regex_str) {
            Ok(regex) => self.regex_to_entity_level.push((regex, level)),
            Err(e) => panic!("Failed to parse regex {regex_str}:\n{e}\n"),
        }
    }

    fn time(&self) -> f64 {
        *self.current_time.lock().unwrap()
    }

    fn set_time(&self, new_time: f64) {
        let mut time_guard = self.current_time.lock().unwrap();
        assert!(new_time >= *time_guard);
        *time_guard = new_time;
    }
}

#[cfg(test)]
mod tests {
    use log::Level;

    use super::*;

    fn entity_paths() -> Vec<&'static str> {
        vec!["top", "top::dev", "top::dev::node0", "top::dev::node1"]
    }

    #[test]
    fn no_filters() {
        let manager = EntityManager::new(Level::Error);

        for p in entity_paths() {
            assert_eq!(manager.log_level_for(p), Level::Error);
        }
    }

    #[test]
    fn filter_dev_trace() {
        let mut manager = EntityManager::new(Level::Error);
        manager.add_entity_level_filter(r".*dev.*", Level::Trace);

        let expected_levels = [Level::Error, Level::Trace, Level::Trace, Level::Trace];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p), expected_levels[i]);
        }
    }

    #[test]
    fn filter_node0_error() {
        let mut manager = EntityManager::new(Level::Warn);
        manager.add_entity_level_filter(r".*node0", Level::Error);

        let expected_levels = [Level::Warn, Level::Warn, Level::Error, Level::Warn];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p), expected_levels[i]);
        }
    }

    #[test]
    fn filter_node0_warn() {
        let mut manager = EntityManager::new(Level::Error);
        manager.add_entity_level_filter(r".*node0", Level::Warn);

        let expected_levels = [Level::Error, Level::Error, Level::Warn, Level::Error];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p), expected_levels[i]);
        }
    }

    #[test]
    fn filter_dev_and_node0_info() {
        let mut manager = EntityManager::new(Level::Error);
        // The first pattern seen should be highest priority
        manager.add_entity_level_filter(r".*node0", Level::Warn);
        manager.add_entity_level_filter(r".*dev.*", Level::Info);

        let expected_levels = [Level::Error, Level::Info, Level::Warn, Level::Info];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p), expected_levels[i]);
        }
    }

    #[test]
    fn filter_log_dev_and_node0_info() {
        let mut manager = EntityManager::new(Level::Error);
        // The first pattern seen should be highest priority
        manager.add_entity_level_filter(r".*node0", Level::Info);
        manager.add_entity_level_filter(r".*dev.*", Level::Trace);
        manager.add_entity_level_filter(r"top.*", Level::Warn);

        let expected_levels = [Level::Warn, Level::Trace, Level::Info, Level::Trace];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p), expected_levels[i]);
        }
    }

    #[test]
    fn tags() {
        let manager = EntityManager::new(Level::Error);
        for i in 0..10 {
            assert_eq!(manager.unique_tag(), Tag(i + ROOT.0 + 1));
        }
    }
}
