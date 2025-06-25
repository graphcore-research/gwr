// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! This module provides helper functions for testing logging output
//!
//! The aim of this module is to provide commonly-used functions that enable the
//! testing of the output that should appear from logging macros.
//!
//! *Note:* all tests should be run in a [serial](https://docs.rs/serial_test) manner because
//! the logger involves shared global state that will otherwise give
//! unpredictable results.

use core::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

use regex::Regex;

use crate::{Tag, Track};

/// A tracker that keeps track events.
pub struct TestTracker {
    events: Mutex<Vec<String>>,

    unique_tag: AtomicU64,
}

impl TestTracker {
    /// Create a new [`Tracker`](crate::Tracker) for the tests.
    ///
    /// This keeps the track events in memory for checking later.
    #[must_use]
    pub fn new(initial_tag: u64) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            unique_tag: AtomicU64::new(initial_tag),
        }
    }

    fn add_event(&self, event: String) {
        println!("{event}");
        let mut events = self.events.lock().unwrap();
        events.push(event);
    }
}

impl Track for TestTracker {
    fn unique_tag(&self) -> Tag {
        let tag = self.unique_tag.fetch_add(1, Ordering::SeqCst);
        Tag(tag)
    }

    fn is_entity_enabled(&self, _tag: Tag, _level: log::Level) -> bool {
        true
    }

    fn add_entity(&self, _tag: Tag, _entity_name: &str) {
        // Do nothing
    }

    fn enter(&self, tag: Tag, item: Tag) {
        self.add_event(format!("{tag}: {item} entered"));
    }

    fn exit(&self, tag: Tag, item: Tag) {
        self.add_event(format!("{tag}: {item} exited"));
    }

    fn create(&self, created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        self.add_event(format!(
            "{created_by}: created {tag}, {name}, {req_type}, {num_bytes} bytes"
        ));
    }

    fn destroy(&self, destroyed_by: Tag, tag: Tag) {
        self.add_event(format!("{destroyed_by}: destroyed {tag}"));
    }

    fn connect(&self, connect_from: Tag, connect_to: Tag) {
        self.add_event(format!("{connect_from}: connect to {connect_to}"));
    }

    fn log(&self, tag: Tag, level: log::Level, msg: std::fmt::Arguments) {
        self.add_event(format!("{tag}:{level}: {msg}"));
    }

    fn time(&self, set_by: Tag, time_ns: f64) {
        self.add_event(format!("{set_by}: set time {time_ns:.1}ns"));
    }

    fn shutdown(&self) {
        // Do nothing
    }
}

/// Initialise the logging system for tests
///
/// Install the logger that will capture all _log_ messages. This is done by
/// setting the default logging level to Trace and installing a logger that
/// records all _log_ messages to a global string.
///
/// *Note*: this is called `test_init` because macros are exported at the root
/// of the crate.
///
/// # Arguments
///
/// * `start_tag` - The tag value to be set as the starting value
///
/// # Examples
///
/// ```
/// use serial_test::serial;
/// use steam_track::test_helpers;
///
/// # /* Need to comment this out so that it is actually built/tested by the infrastructure
/// #[test]
/// # */
/// fn smoke() {
///     let (test_tracker, tracker) = steam_track::test_init!(10);
///     let top = steam_track::entity::toplevel(&tracker, "top");
///     test_helpers::check_and_clear(&test_tracker, &["10: top created"]);
/// }
/// ```
#[macro_export]
macro_rules! test_init {
    ($start_tag:expr) => {{
        let test_tracker = std::sync::Arc::new($crate::test_helpers::TestTracker::new($start_tag));
        let tracker: $crate::Tracker = test_tracker.clone();
        (test_tracker, tracker)
    }};
}

/// Check and clear the _trace_ and _log_ output
///
/// This function asserts that the logging output lines seen since the start or
/// the last time this function was called are expected. The
/// [test_init](../../steam_track/macro.test_init.html) must have been called
/// before this function can be used.
///
/// It then also clears both the _trace_ and _log_ output recorded so far.
///
/// # Arguments
///
/// * `tracker`  - A reference to the [`TestTracker`] being used in the test.
///   This will have been keeping track of the trace and log events seen since
///   it was created or last cleared.
/// * `expected` - An array of expected regular expressions that the logging
///   output will be matched against.
///
/// # Examples
///
/// ```
/// use serial_test::serial;
/// use steam_track::test_helpers;
///
/// # /* Need to comment this out so that it is actually built/tested by the infrastructure
/// #[test]
/// # */
/// fn smoke() {
///     let (test_tracker, tracker) = steam_track::test_init!(20);
///     let top = steam_track::entity::toplevel(&tracker, "top");
///     let tag = steam_track::create_tag!(top);
///     test_helpers::check_and_clear(&test_tracker, &["20: top created"]);
/// }
/// ```
pub fn check_and_clear(tracker: &TestTracker, expected: &[&str]) {
    let mut log_contents_ref = tracker.events.lock().unwrap();

    println!("Checking {:?} matches {:?}", expected, *log_contents_ref);

    // Check that there are the same number of strings produced as expected
    let num_strings = expected.len();
    assert_eq!(num_strings, log_contents_ref.len());

    for i in 0..num_strings {
        let log_expect = expected[i];
        let re = Regex::new(log_expect).unwrap();
        let actual = &(*log_contents_ref[i]);
        println!("Checking {i}: {log_expect:?} matches {actual:?}");
        assert!(re.is_match(actual));
    }

    log_contents_ref.clear();
}
