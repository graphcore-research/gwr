// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! This module provides combined _track_ capabilities for the STEAM project.
//!
//! _Track_ means the combination of _log_ and _trace_ where:
//!
//!   - _log_ are text-based human-readable messages emitted at various levels
//!     of verbosity (from `Trace` through to `Error`).
//!   - _trace_ provides a standard set of modelling events that can be emitted.
//!     For example, object creation/destruction or objects entering/exitting
//!     simulation [`Entities`](crate::entity::Entity).
//!
//! The _track_ events can be emitted using:
//!
//!   - a textual output based on the [log](https://docs.rs/log) crate.
//!   - a packed binary output based on [Cap'n Proto](https://capnproto.org/).

// Enable warnings for missing documentation
#![warn(missing_docs)]

use std::str::FromStr;
use std::sync::{Arc, Mutex};

pub use log;

pub mod entity;
pub mod tag;

/// Include the trackers.
pub mod tracker;
pub use tracker::{Track, Tracker};

/// A type alias for objects that receive _log_ / _trace_ events.
///
/// The writer must implement Send in order to be shared between threads.
pub type Writer = Box<dyn std::io::Write + Send>;
type SharedWriter = Arc<Mutex<Writer>>;

/// Take the command-line string and convert it to a Level
#[must_use]
pub fn str_to_level(lvl: &str) -> log::Level {
    match log::Level::from_str(lvl) {
        Ok(level) => level,
        Err(_) => panic!("Unable to parse level string '{lvl}'"),
    }
}

/// Type used for unique tags
///
/// Each _log_/_trace_ event within the application is given a unique tag to
/// identify it. There are two reserved tag values: [NO_ID](constant.NO_ID.html)
/// and [ROOT](constant.ROOT.html)
pub use tag::Tag;

pub mod test_helpers;
pub mod trace_visitor;

/// Tag value which indicates where there is no valid tag
pub const NO_ID: Tag = tag::Tag(0);

/// The root tag from which all other tags are derived
pub const ROOT: Tag = tag::Tag(1);

// Track an enter event.
#[doc(hidden)]
#[macro_export]
macro_rules! enter {
    ($entity:expr ; $enter_tag:expr) => {
        if $entity
            .tracker
            .is_entity_enabled($entity.tag, log::Level::Trace)
        {
            $entity.tracker.enter($entity.tag, $enter_tag);
        }
    };
}

// Track an exit event.
#[doc(hidden)]
#[macro_export]
macro_rules! exit {
    ($entity:expr ; $exit_tag:expr) => {
        if $entity
            .tracker
            .is_entity_enabled($entity.tag, log::Level::Trace)
        {
            $entity.tracker.exit($entity.tag, $exit_tag);
        }
    };
}

/// Create a unique tag for tracking.
///
/// The user must specify an entity with a [`Tracker`] to create the tag.
///
/// **Note:** this macro should be used when the object being assigned the
///           [`Tag`] will have its creation tracked with [`create`].
#[macro_export]
macro_rules! create_tag {
    ($entity:expr) => {{ $entity.tracker.unique_tag() }};
}

/// Create a unique tag for tracking and track the creation.
///
/// The user must specify an entity with a [`Tracker`] to create the tag.
/// The creation event will be tracked if the entity has trace enabled.
///
/// **Note:** this macro should only be used if the object being assigned the
///           [`Tag`] will not have its creation tracked with [`create`].
#[macro_export]
macro_rules! create_and_track_tag {
    ($entity:expr) => {{
        let tag = $entity.tracker.unique_tag();
        if $entity
            .tracker
            .is_entity_enabled($entity.tag, log::Level::Trace)
        {
            $entity.tracker.create($entity.tag, tag, 0, 0, "tag");
        }
        tag
    }};
}

/// Destroy a tag
///
/// Destroying a tag indicates to the logging system that this tag is finished
/// with and should therefore not be used any more. This is not enforced at
/// runtime, and therefore will not cause any errors to be reported if it is
/// used.
#[macro_export]
macro_rules! destroy_tag {
    ($entity:expr ; $tag:expr) => {{
        if $entity
            .tracker
            .is_entity_enabled($entity.tag, log::Level::Trace)
        {
            $entity.tracker.destroy($entity.tag, $tag);
        }
    }};
}

/// Add an entity creation event
#[macro_export]
macro_rules! create {
    ($entity:expr) => {{
        if $entity
            .tracker
            .is_entity_enabled($entity.tag, log::Level::Trace)
        {
            let parent_tag = match &$entity.parent {
                Some(parent) => parent.tag,
                None => $crate::NO_ID,
            };
            $entity
                .tracker
                .create(parent_tag, $entity.tag, 0, 0, $entity.full_name().as_str());
        }
    }};
    ($entity:expr ; $created:expr, $num_bytes:expr, $req_type:expr) => {{
        if $entity
            .tracker
            .is_entity_enabled($entity.tag, log::Level::Trace)
        {
            $entity.tracker.create(
                $entity.tag,
                $created.tag,
                $num_bytes,
                $req_type,
                format!("{}", $created).as_str(),
            );
        }
    }};
}

/// Add an entity destroy event
#[macro_export]
macro_rules! destroy {
    ($entity:expr) => {{
        if $entity
            .tracker
            .is_entity_enabled($entity.tag, log::Level::Trace)
        {
            match &$entity.parent {
                Some(parent) => $entity.tracker.destroy($entity.tag, parent.tag),
                None => $entity.tracker.destroy($entity.tag, $crate::NO_ID),
            };
        }
    }};
}

/// Connect two entities
#[macro_export]
macro_rules! connect {
    ($from_entity:expr ; $to_entity:expr) => {{
        if $from_entity
            .tracker
            .is_entity_enabled($from_entity.tag, log::Level::Trace)
        {
            $from_entity
                .tracker
                .connect($from_entity.tag, $to_entity.tag);
        }
    }};
}

/// Update the current time.
#[macro_export]
macro_rules! set_time {
    ($entity:expr ; $time_ns:expr) => {{
        if $entity
            .tracker
            .is_entity_enabled($entity.tag, log::Level::Trace)
        {
            $entity.tracker.time($entity.tag, $time_ns);
        }
    }};
}

/// Base macro for log messages of all level.
///
/// This wrapper calls both the [`log`](https://docs.rs/log)::log macro and also the
/// [`Trace`](trait.Trace.html) [message](trait.Trace.html#tymethod.message)
/// function which will emit `message` tracking events to the Cap'n Proto binary
/// stream.
#[macro_export]
macro_rules! log_base {
    ($entity:expr ; $lvl:expr, $($arg:tt)+) => (
        if $entity.tracker.is_entity_enabled($entity.tag, $lvl) {
            $entity.tracker.log($entity.tag, $lvl, format_args!($($arg)+));
        }
    );
}

/// The `trace` macro provides a wrapper for the [`log`](macro.log.html) macro
/// at level `log::Level::Trace`
#[macro_export]
macro_rules! trace {
    ($entity:expr ; $($arg:tt)+) => (
        $crate::log_base!($entity ; $crate::log::Level::Trace, $($arg)+);
    );
}

/// The `debug` macro provides a wrapper for the [`log`](macro.log.html) macro
/// at level `log::Level::Debug`
#[macro_export]
macro_rules! debug {
    ($entity:expr ; $($arg:tt)+) => (
        $crate::log_base!($entity ; $crate::log::Level::Debug, $($arg)+);
    );
}

/// The `info` macro provides a wrapper for the [`log`](macro.log.html) macro at
/// level `log::Level::Info`
#[macro_export]
macro_rules! info {
    ($entity:expr ; $($arg:tt)+) => (
        $crate::log_base!($entity ; $crate::log::Level::Info, $($arg)+);
    );
}

/// The `warn` macro provides a wrapper for the [`log`](macro.log.html) macro at
/// level `log::Level::Info`
#[macro_export]
macro_rules! warn {
    ($entity:expr ; $($arg:tt)+) => (
        $crate::log_base!($entity ; $crate::log::Level::Warn, $($arg)+);
    );
}

/// the `error` macro provides a wrapper for the [`log`](macro.log.html) macro
/// at level `log::Level::Error`
#[macro_export]
macro_rules! error {
    ($entity:expr ; $($arg:tt)+) => (
        $crate::log_base!($entity ; $crate::log::Level::Error, $($arg)+);
    );
}

/// Auto-generated [Cap'n Proto](https://capnproto.org/) module
///
/// The contents of this file are created by `build.rs` at compile-time. They
/// provide all the functions required to build up
/// [Cap'n Proto](https://capnproto.org/) events as defined in the
/// `schemas/steam_trace.capnp` file.
pub mod steam_track_capnp {
    // No need to emit warnings for auto-generated Cap'n Proto code
    #![allow(missing_docs)]
    #![allow(clippy::pedantic)]

    include!(concat!(env!("OUT_DIR"), "/steam_track_capnp.rs"));
}
