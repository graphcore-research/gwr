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

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub use capnp::CapnProtoTracker;
pub use dev_null::DevNullTracker;
pub use in_memory::InMemoryTracker;
use regex::Regex;
pub use text::TextTracker;

use crate::{ROOT, Tag, TraceState};

/// This is the interface that is supported by all [`Tracker`]s.
pub trait Track {
    /// Allocate a new global tag
    fn unique_tag(&self) -> Tag;

    /// Determine whether tracing is enabled and what the log level is for an
    /// entity.
    fn get_entity_enables(&self, entity_name: &str) -> (bool, log::Level);

    /// Track when an object with the given tag arrives.
    fn enter(&self, enter_into: Tag, enter_obj: Tag);

    /// Track when an object with the given tag leaves.
    fn exit(&self, exit_from: Tag, exit_obj: Tag);

    /// Track when an object with the given tag is created.
    fn create(&self, created_by: Tag, created_obj: Tag, num_bytes: usize, req_type: i8, name: &str);

    /// Track when an object with the given tag is destroyed.
    fn destroy(&self, destroyed_by: Tag, destroyed_obj: Tag);

    /// Track a log message of the given level.
    fn log(&self, msg_by: Tag, level: log::Level, msg: std::fmt::Arguments);

    /// Advance the time to the time specified in `ns`.
    fn time(&self, set_by: Tag, time_ns: f64);
}

/// The type of a [`Tracker`] that is shared across entities.
pub type Tracker = Arc<dyn Track + Send + Sync>;

/// Create a [`Tracker`] that prints all track events to `stdout`.
pub fn stdout_tracker() -> Tracker {
    let entity_manger = Arc::new(EntityManager::new(TraceState::Enabled, log::Level::Warn));
    let stdout_writer = Box::new(std::io::BufWriter::new(io::stdout()));
    let tracer: Tracker = Arc::new(TextTracker::new(entity_manger.clone(), stdout_writer));
    tracer
}

/// Create a [`Tracker`] that suppresses all track events.
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
    /// Level of _log_ events to output.
    default_trace_enabled: bool,

    /// Level of _log_ events to output.
    default_log_level: log::Level,

    /// List of regular expressions mapping entity names to trace
    /// enable/disable.
    regex_to_trace_enabled: Vec<(Regex, bool)>,

    /// List of regular expressions mapping entity names to log levels.
    regex_to_log_level: Vec<(Regex, log::Level)>,

    /// Used to assign unique tags.
    unique_tag: AtomicU64,

    /// Keep track of the current time.
    current_time: Mutex<f64>,
}

impl EntityManager {
    /// Constructor with [`TraceState`] and [`log::Level`]
    pub fn new(default_trace_enabled: TraceState, default_log_level: log::Level) -> Self {
        Self {
            default_trace_enabled: default_trace_enabled == TraceState::Enabled,
            default_log_level,
            regex_to_trace_enabled: Vec::new(),
            regex_to_log_level: Vec::new(),
            unique_tag: AtomicU64::new(ROOT.0 + 1),
            current_time: Mutex::new(0.0),
        }
    }

    fn unique_tag(&self) -> Tag {
        let tag = self.unique_tag.fetch_add(1, Ordering::SeqCst);
        Tag(tag)
    }

    fn trace_enabled_for(&self, entity_name: &str) -> bool {
        for (regex, enabled) in self.regex_to_trace_enabled.iter() {
            if regex.is_match(entity_name) {
                return *enabled;
            }
        }
        self.default_trace_enabled
    }

    fn log_level_for(&self, entity_name: &str) -> log::Level {
        for (regex, level) in self.regex_to_log_level.iter() {
            if regex.is_match(entity_name) {
                return *level;
            }
        }
        self.default_log_level
    }

    /// Add a log filter regular expression.
    ///
    /// # Example
    ///
    /// ```rust
    /// use steam_track::TraceState;
    /// use steam_track::tracker::EntityManager;
    /// let mut manager = EntityManager::new(TraceState::Disabled, log::Level::Warn);
    /// manager.add_log_filter(".*arb.*", log::Level::Trace);
    /// ```
    pub fn add_log_filter(&mut self, regex_str: &str, level: crate::log::Level) {
        match Regex::new(regex_str) {
            Ok(regex) => self.regex_to_log_level.push((regex, level)),
            Err(e) => panic!("Failed to parse regex {regex_str}:\n{}\n", e),
        };
    }

    /// Add a filter regular expression for enabling/disabling trace for
    /// matching entities.
    ///
    /// # Example
    ///
    /// ```rust
    /// use steam_track::TraceState;
    /// use steam_track::tracker::EntityManager;
    /// let mut manager = EntityManager::new(TraceState::Disabled, log::Level::Warn);
    /// manager.add_trace_filter(".*arb.*", TraceState::Enabled);
    /// ```
    pub fn add_trace_filter(&mut self, regex_str: &str, enabled: TraceState) {
        match Regex::new(regex_str) {
            Ok(regex) => self
                .regex_to_trace_enabled
                .push((regex, enabled == TraceState::Enabled)),
            Err(e) => panic!("Failed to parse regex {regex_str}:\n{}\n", e),
        };
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
    use super::*;

    fn entity_paths() -> Vec<&'static str> {
        vec!["top", "top::rho", "top::rho::tn0", "top::rho::tn1"]
    }

    #[test]
    fn no_filters() {
        let manager = EntityManager::new(TraceState::Disabled, log::Level::Error);

        for p in entity_paths() {
            assert!(!manager.trace_enabled_for(p));
            assert_eq!(manager.log_level_for(p), log::Level::Error);
        }
    }

    #[test]
    fn filter_trace_rho_enable() {
        let mut manager = EntityManager::new(TraceState::Disabled, log::Level::Error);
        manager.add_trace_filter(r".*rho.*", TraceState::Enabled);

        let expected_enables = [false, true, true, true];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.trace_enabled_for(p), expected_enables[i]);
        }
    }

    #[test]
    fn filter_trace_tn0_enable() {
        let mut manager = EntityManager::new(TraceState::Disabled, log::Level::Error);
        manager.add_trace_filter(r".*tn0", TraceState::Enabled);

        let expected_enables = [false, false, true, false];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.trace_enabled_for(p), expected_enables[i]);
        }
    }

    #[test]
    fn filter_trace_tn0_disable() {
        let mut manager = EntityManager::new(TraceState::Enabled, log::Level::Error);
        manager.add_trace_filter(r".*tn0", TraceState::Disabled);

        let expected_enables = [true, true, false, true];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.trace_enabled_for(p), expected_enables[i]);
        }
    }

    #[test]
    fn filter_trace_rho_and_tn0_disable() {
        let mut manager = EntityManager::new(TraceState::Enabled, log::Level::Error);
        // The first pattern seen should be highest priority
        manager.add_trace_filter(r".*tn0", TraceState::Enabled);
        manager.add_trace_filter(r".*rho.*", TraceState::Disabled);

        let expected_enables = [true, false, true, false];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.trace_enabled_for(p), expected_enables[i]);
        }
    }

    #[test]
    fn filter_log_rho_and_tn0_disable() {
        let mut manager = EntityManager::new(TraceState::Enabled, log::Level::Error);
        // The first pattern seen should be highest priority
        manager.add_log_filter(r".*tn0", log::Level::Info);
        manager.add_log_filter(r".*rho.*", log::Level::Trace);
        manager.add_log_filter(r"top.*", log::Level::Warn);

        let expected_levels = [
            log::Level::Warn,
            log::Level::Trace,
            log::Level::Info,
            log::Level::Trace,
        ];

        for (i, p) in entity_paths().iter().enumerate() {
            assert_eq!(manager.log_level_for(p), expected_levels[i]);
        }
    }

    #[test]
    fn tags() {
        let manager = EntityManager::new(TraceState::Disabled, log::Level::Error);
        for i in 0..10 {
            assert_eq!(manager.unique_tag(), Tag(i + ROOT.0 + 1));
        }
    }
}
