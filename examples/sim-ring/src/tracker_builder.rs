// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
//
//! Library functions to build trackers as defined by the user.

use std::io::BufWriter;
use std::sync::Arc;
use std::{fs, io};

use steam_track::tracker::multi_tracker::MultiTracker;
use steam_track::tracker::{CapnProtoTracker, EntityManager, TextTracker, TrackConfigError};
use steam_track::{Tracker, Writer};

/// Create a tracker that prints to stdout
///
/// The user can pass a filter regular expression which will set the level only
/// for matching Entities and set all other Entities to only emit errors.
fn build_stdout_tracker(
    level: log::Level,
    filter_regex: &str,
) -> Result<Tracker, TrackConfigError> {
    let default_level = if filter_regex.is_empty() {
        level
    } else {
        log::Level::Error
    };
    let mut entity_manager = EntityManager::new(default_level);
    if !filter_regex.is_empty() {
        entity_manager.add_entity_level_filter(filter_regex, level)?;
    }
    let stdout_writer = Box::new(std::io::BufWriter::new(io::stdout()));
    Ok(Arc::new(TextTracker::new(entity_manager, stdout_writer)))
}

/// Same as the text tracker (see build_stdout_tracker) except will generate a
/// binary file.
fn build_binary_tracker(
    level: log::Level,
    filter_regex: &str,
    trace_file: &str,
) -> Result<Tracker, TrackConfigError> {
    let default_level = if filter_regex.is_empty() {
        level
    } else {
        log::Level::Error
    };
    let mut entity_manager = EntityManager::new(default_level);
    if !filter_regex.is_empty() {
        entity_manager.add_entity_level_filter(filter_regex, level)?;
    }

    let bin_writer: Writer = Box::new(BufWriter::new(fs::File::create(trace_file).unwrap()));
    Ok(Arc::new(CapnProtoTracker::new(entity_manager, bin_writer)))
}

/// Set up stdout/binary trackers according the the command-line arguments
pub fn setup_trackers(
    enable_stdout: bool,
    stdout_level: log::Level,
    stdout_filter_regex: &str,
    enable_binary: bool,
    binary_level: log::Level,
    binary_filter_regex: &str,
    binary_file: &str,
) -> Result<Tracker, TrackConfigError> {
    if enable_stdout && enable_binary {
        let mut tracker = MultiTracker::default();

        let log_tracker: Tracker = build_stdout_tracker(stdout_level, stdout_filter_regex)?;
        let trace_tracker: Tracker =
            build_binary_tracker(binary_level, binary_filter_regex, binary_file)?;
        tracker.add_tracker(log_tracker);
        tracker.add_tracker(trace_tracker);
        Ok(Arc::new(tracker))
    } else if enable_stdout {
        build_stdout_tracker(stdout_level, stdout_filter_regex)
    } else if enable_binary {
        build_binary_tracker(binary_level, binary_filter_regex, binary_file)
    } else {
        build_stdout_tracker(log::Level::Warn, "")
    }
}
