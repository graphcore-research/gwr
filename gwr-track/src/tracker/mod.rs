// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Define the [`Track`] trait a number of [`Tracker`]s.

/// Include the alternative name manager.
pub mod aka;
/// Include the CapnProto tracker.
pub mod capnp;
/// Include the /dev/null tracker.
pub mod dev_null;
/// Include the in-memory tracker.
pub mod in_memory;
#[cfg(feature = "perfetto")]
/// Include the Perfetto tracker.
pub mod perfetto;
/// Include the text-based tracker.
pub mod text;
/// Include the types required for tracker.
pub mod types;

/// Include the multi-tracker.
pub mod multi_tracker;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::rc::Rc;

pub use capnp::CapnProtoTracker;
pub use dev_null::DevNullTracker;
pub use in_memory::InMemoryTracker;
use regex::Regex;
pub use text::TextTracker;

use crate::tracker::aka::AlternativeNames;
use crate::{Id, ROOT};

/// Error used to return configuration errors
#[derive(Debug)]
pub struct TrackConfigError(pub String);

/// This is the interface that is supported by all [`Tracker`]s.
pub trait Track {
    /// Allocate a new global ID
    fn unique_id(&self) -> Id;

    /// Determine whether tracking is enabled, and at what level for an
    /// entity looked up by its ID.
    fn is_entity_enabled(&self, id: Id, level: log::Level) -> bool;

    /// Return the monitoring window size if it is to be enabled.
    /// Entity looked up by its ID.
    fn monitoring_window_size_for(&self, id: Id) -> Option<u64>;

    /// Record an entity being created.
    fn add_entity(&self, id: Id, entity_name: &str, alternative_names: AlternativeNames);

    /// Track when an entity with the given ID arrives.
    fn enter(&self, enter_into: Id, enter_obj: Id);

    /// Track when an entity with the given ID leaves.
    fn exit(&self, exit_from: Id, exit_obj: Id);

    /// Track an entity setting a value.
    fn value(&self, id: Id, value: f64);

    /// Track when an entity with the given ID is created.
    fn create(&self, created_by: Id, created_obj: Id, num_bytes: usize, req_type: i8, name: &str);

    /// Track when an entity with the given ID is destroyed.
    fn destroy(&self, destroyed_by: Id, destroyed_obj: Id);

    /// Track when an entity is connected to another entity
    fn connect(&self, connect_from: Id, connect_to: Id);

    /// Track a log message of the given level.
    fn log(&self, msg_by: Id, level: log::Level, msg: std::fmt::Arguments);

    /// Advance the time to the time specified in `ns`.
    fn time(&self, set_by: Id, time_ns: f64);

    /// Perform any pre-exit shutdown/cleanup
    fn shutdown(&self);
}

/// The type of a [`Tracker`] that is shared across entities.
pub type Tracker = Rc<dyn Track>;

/// Create a [`Tracker`] that prints all track events to `stdout`.
#[must_use]
pub fn stdout_tracker(level: log::Level) -> Tracker {
    let entity_manger = EntityManager::new(level);
    let stdout_writer = Box::new(std::io::BufWriter::new(io::stdout()));
    let tracker: Tracker = Rc::new(TextTracker::new(entity_manger, stdout_writer));
    tracker
}

/// Create a [`Tracker`] that suppresses all track events.
#[must_use]
pub fn dev_null_tracker() -> Tracker {
    let tracer: Tracker = Rc::new(DevNullTracker {});
    tracer
}

/// The [`EntityManager`] is responsible for determining entity log / trace
/// enable states.
///
/// This is shared by the [`Text`](crate::tracker::text) and
/// [`Capnp`](crate::tracker::capnp)-based trackers, as well as the
/// [`Perfetto`](crate::tracker::perfetto) tracker.
///
/// This manager is also used to allocate unique [`Id`] values.
pub struct EntityManager {
    /// Level of tracking events to output.
    default_entity_level: log::Level,

    /// List of regular expressions mapping entity names to log levels.
    regex_to_entity_level: Vec<(Regex, log::Level)>,

    /// List of regular expressions mapping entity names to log levels.
    regex_to_enable_monitors_for: Vec<(Regex, u64)>,

    /// Used to assign unique IDs.
    unique_id: RefCell<u64>,

    /// Keep track of the current time.
    current_time: RefCell<f64>,

    /// Keep track of entities that have trace enable/log levels different to
    /// the default.
    log_entity_lookup: RefCell<HashMap<Id, log::Level>>,

    /// Keep track of the window size for entities.
    monitor_window_size_lookup: RefCell<HashMap<Id, u64>>,
}

impl EntityManager {
    /// Constructor with default [`log::Level`]
    #[must_use]
    pub fn new(default_entity_level: log::Level) -> Self {
        Self {
            default_entity_level,
            regex_to_entity_level: Vec::new(),
            regex_to_enable_monitors_for: Vec::new(),
            unique_id: RefCell::new(ROOT.0 + 1),
            current_time: RefCell::new(0.0),
            log_entity_lookup: RefCell::new(HashMap::new()),
            monitor_window_size_lookup: RefCell::new(HashMap::new()),
        }
    }

    fn unique_id(&self) -> Id {
        let mut guard = self.unique_id.borrow_mut();
        let id = *guard;
        *guard += 1;
        Id(id)
    }

    fn is_log_enabled_at_level(&self, id: Id, level: log::Level) -> bool {
        match self.log_entity_lookup.borrow().get(&id) {
            None => level <= self.default_entity_level,
            Some(entity_level) => level <= *entity_level,
        }
    }

    fn monitoring_window_size_for(&self, id: Id) -> Option<u64> {
        self.monitor_window_size_lookup.borrow().get(&id).copied()
    }

    fn add_entity(&self, id: Id, entity_name: &str, alternative_names: AlternativeNames) {
        let entity_level = self.log_level_for(entity_name, alternative_names);
        if entity_level != self.default_entity_level
            && self
                .log_entity_lookup
                .borrow_mut()
                .insert(id, entity_level)
                .is_some()
        {
            panic!("Entity ID {id} already seen ({entity_name})");
        }

        if let Some(window_size_ticks) =
            self.monitor_window_size_for(entity_name, alternative_names)
        {
            self.monitor_window_size_lookup
                .borrow_mut()
                .insert(id, window_size_ticks);
        }
    }

    fn log_level_for(&self, entity_name: &str, alternative_names: AlternativeNames) -> log::Level {
        for (regex, level) in &self.regex_to_entity_level {
            if regex.is_match(entity_name) {
                return *level;
            }
            if let Some(alternative_names) = alternative_names {
                for name in alternative_names {
                    if regex.is_match(name.as_str()) {
                        return *level;
                    }
                }
            }
        }
        self.default_entity_level
    }

    fn monitor_window_size_for(
        &self,
        entity_name: &str,
        alternative_names: AlternativeNames,
    ) -> Option<u64> {
        for (regex, window_size_ticks) in &self.regex_to_enable_monitors_for {
            if regex.is_match(entity_name) {
                return Some(*window_size_ticks);
            }
            if let Some(alternative_names) = alternative_names {
                for name in alternative_names {
                    if regex.is_match(name.as_str()) {
                        return Some(*window_size_ticks);
                    }
                }
            }
        }
        None
    }

    /// Add a filter regular expression to set matching entites to a given
    /// level.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gwr_track::tracker::EntityManager;
    /// let mut manager = EntityManager::new(log::Level::Warn);
    /// manager.add_entity_level_filter(".*arb.*", log::Level::Trace);
    /// ```
    pub fn add_entity_level_filter(
        &mut self,
        regex_str: &str,
        level: crate::log::Level,
    ) -> Result<(), TrackConfigError> {
        match Regex::new(regex_str) {
            Ok(regex) => self.regex_to_entity_level.push((regex, level)),
            Err(e) => {
                return Err(TrackConfigError(format!(
                    "Failed to parse regex {regex_str}:\n{e}\n"
                )));
            }
        }
        Ok(())
    }

    /// Add a filter for ports that should have monitoring enabled
    /// with the specified window size in ticks.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gwr_track::tracker::EntityManager;
    /// let mut manager = EntityManager::new(log::Level::Warn);
    /// manager.set_monitor_window_size_for(".*fabric::ingress.*", 250);
    /// ```
    pub fn set_monitor_window_size_for(
        &mut self,
        regex_str: &str,
        window_size_ticks: u64,
    ) -> Result<(), TrackConfigError> {
        match Regex::new(regex_str) {
            Ok(regex) => self
                .regex_to_enable_monitors_for
                .push((regex, window_size_ticks)),
            Err(e) => {
                return Err(TrackConfigError(format!(
                    "Failed to parse regex {regex_str}:\n{e}\n"
                )));
            }
        }
        Ok(())
    }

    fn time(&self) -> f64 {
        *self.current_time.borrow()
    }

    fn set_time(&self, new_time: f64) {
        let mut time_guard = self.current_time.borrow_mut();
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
            assert_eq!(manager.log_level_for(p, None), Level::Error);
        }
    }

    #[test]
    fn filter_dev_trace() {
        let mut manager = EntityManager::new(Level::Error);
        manager
            .add_entity_level_filter(r".*dev.*", Level::Trace)
            .unwrap();

        let expected_levels = [Level::Error, Level::Trace, Level::Trace, Level::Trace];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p, None), expected_levels[i]);
        }
    }

    #[test]
    fn filter_node0_error() {
        let mut manager = EntityManager::new(Level::Warn);
        manager
            .add_entity_level_filter(r".*node0", Level::Error)
            .unwrap();

        let expected_levels = [Level::Warn, Level::Warn, Level::Error, Level::Warn];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p, None), expected_levels[i]);
        }
    }

    #[test]
    fn filter_node0_warn() {
        let mut manager = EntityManager::new(Level::Error);
        manager
            .add_entity_level_filter(r".*node0", Level::Warn)
            .unwrap();

        let expected_levels = [Level::Error, Level::Error, Level::Warn, Level::Error];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p, None), expected_levels[i]);
        }
    }

    #[test]
    fn filter_dev_and_node0_info() {
        let mut manager = EntityManager::new(Level::Error);
        // The first pattern seen should be highest priority
        manager
            .add_entity_level_filter(r".*node0", Level::Warn)
            .unwrap();
        manager
            .add_entity_level_filter(r".*dev.*", Level::Info)
            .unwrap();

        let expected_levels = [Level::Error, Level::Info, Level::Warn, Level::Info];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p, None), expected_levels[i]);
        }
    }

    #[test]
    fn filter_log_dev_and_node0_info() {
        let mut manager = EntityManager::new(Level::Error);
        // The first pattern seen should be highest priority
        manager
            .add_entity_level_filter(r".*node0", Level::Info)
            .unwrap();
        manager
            .add_entity_level_filter(r".*dev.*", Level::Trace)
            .unwrap();
        manager
            .add_entity_level_filter(r"top.*", Level::Warn)
            .unwrap();

        let expected_levels = [Level::Warn, Level::Trace, Level::Info, Level::Trace];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p, None), expected_levels[i]);
        }
    }

    #[test]
    fn ids() {
        let manager = EntityManager::new(Level::Error);
        for i in 0..10 {
            assert_eq!(manager.unique_id(), Id(i + ROOT.0 + 1));
        }
    }
}
