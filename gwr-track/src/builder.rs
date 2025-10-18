// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Library functions to build trackers as defined by the user.

use std::io::BufWriter;
use std::rc::Rc;
use std::{fs, io};

use crate::tracker::multi_tracker::MultiTracker;
#[cfg(feature = "perfetto")]
use crate::tracker::perfetto::PerfettoTracker;
use crate::tracker::{CapnProtoTracker, EntityManager, TextTracker, TrackConfigError};
use crate::{Tracker, Writer};

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
    Ok(Rc::new(TextTracker::new(entity_manager, stdout_writer)))
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
    Ok(Rc::new(CapnProtoTracker::new(entity_manager, bin_writer)))
}

/// This tracker will produce a Perfetto trace file, which unlike the other
/// tracker options can be viewed using the Perfetto UI, rather than
/// gwr-spotter.
#[cfg(feature = "perfetto")]
fn build_perfetto_tracker(
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
    Ok(Rc::new(PerfettoTracker::new(entity_manager, bin_writer)))
}

#[cfg(not(feature = "perfetto"))]
/// Simply have this here to make it easier to write the `setup_trackers`
/// function
fn build_perfetto_tracker(
    _level: log::Level,
    _filter_regex: &str,
    _trace_file: &str,
) -> Result<Tracker, TrackConfigError> {
    panic!("Cannot build perfetto tracker without feature");
}

/// Set up stdout/binary/Perfetto trackers according the the command-line
/// arguments
#[expect(clippy::too_many_arguments)]
fn setup_trackers(
    enable_stdout: bool,
    stdout_level: log::Level,
    stdout_filter_regex: &str,
    enable_binary: bool,
    binary_level: log::Level,
    binary_filter_regex: &str,
    binary_file: &str,
    enable_perfetto: bool,
    perfetto_level: log::Level,
    perfetto_filter_regex: &str,
    perfetto_file: &str,
) -> Result<Tracker, TrackConfigError> {
    let multi_tracker_required = [enable_stdout, enable_binary, enable_perfetto]
        .into_iter()
        .filter(|x| *x)
        .count()
        > 1;

    if multi_tracker_required {
        let mut tracker = MultiTracker::default();

        if enable_stdout {
            let log_tracker: Tracker = build_stdout_tracker(stdout_level, stdout_filter_regex)?;
            tracker.add_tracker(log_tracker);
        }
        if enable_binary {
            let trace_tracker: Tracker =
                build_binary_tracker(binary_level, binary_filter_regex, binary_file)?;
            tracker.add_tracker(trace_tracker);
        }
        if enable_perfetto {
            let perfetto_tracker: Tracker =
                build_perfetto_tracker(perfetto_level, perfetto_filter_regex, perfetto_file)?;
            tracker.add_tracker(perfetto_tracker);
        }

        Ok(Rc::new(tracker))
    } else if enable_stdout {
        build_stdout_tracker(stdout_level, stdout_filter_regex)
    } else if enable_binary {
        build_binary_tracker(binary_level, binary_filter_regex, binary_file)
    } else if enable_perfetto {
        build_perfetto_tracker(perfetto_level, perfetto_filter_regex, perfetto_file)
    } else {
        build_stdout_tracker(log::Level::Warn, "")
    }
}

/// Set up stdout and binary trackers according the the command-line arguments
pub fn setup_stdout_binary_trackers(
    enable_stdout: bool,
    stdout_level: log::Level,
    stdout_filter_regex: &str,
    enable_binary: bool,
    binary_level: log::Level,
    binary_filter_regex: &str,
    binary_file: &str,
) -> Result<Tracker, TrackConfigError> {
    setup_trackers(
        enable_stdout,
        stdout_level,
        stdout_filter_regex,
        enable_binary,
        binary_level,
        binary_filter_regex,
        binary_file,
        false,
        log::Level::Error,
        "",
        "",
    )
}

/// Set up stdout/binary/Perfetto trackers according the the command-line
/// arguments
///
/// Only available if building with the prefetto feature enabled.
#[expect(clippy::too_many_arguments)]
#[cfg(feature = "perfetto")]
pub fn setup_all_trackers(
    enable_stdout: bool,
    stdout_level: log::Level,
    stdout_filter_regex: &str,
    enable_binary: bool,
    binary_level: log::Level,
    binary_filter_regex: &str,
    binary_file: &str,
    enable_perfetto: bool,
    perfetto_level: log::Level,
    perfetto_filter_regex: &str,
    perfetto_file: &str,
) -> Result<Tracker, TrackConfigError> {
    setup_trackers(
        enable_stdout,
        stdout_level,
        stdout_filter_regex,
        enable_binary,
        binary_level,
        binary_filter_regex,
        binary_file,
        enable_perfetto,
        perfetto_level,
        perfetto_filter_regex,
        perfetto_file,
    )
}
