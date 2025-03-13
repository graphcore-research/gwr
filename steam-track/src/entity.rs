// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A simulation entity.
//!
//! All parts of a model should contain an entity in order to maintain a
//! hierarchy of simulation entities. They contain a name and a unique tag
//! for tracing.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::fmt;
use std::sync::Arc;

use crate::{Tag, Tracker, create, destroy};

/// A simulation entity
///
/// An entity is a part of a hierarchical simulation in which it must have a
/// parent. The simulation top-level should be created using `toplevel("name")`.
///
/// The entity is used when logging so that its unique tag can be emitted and
/// it can determine which messages are emitted to both the binary and textual
/// outputs.
pub struct Entity {
    /// Name of this entity.
    pub name: String,

    /// Optional parent entity (only the top-level should be None).
    pub parent: Option<Arc<Entity>>,

    /// Unique simulation identifier used for bin/log messages.
    pub tag: Tag,

    /// Determines the level of logging messages emitted for this entity.
    log_level: AtomicUsize,

    /// Determines whether trace events are enabled for this entity.
    trace_enabled: AtomicBool,

    /// [`Tracker`] used to handle trace/log events.
    pub tracker: Tracker,
}

static JOIN: &str = "::";

impl Entity {
    /// Create a new entity.
    pub fn new(parent: &Arc<Entity>, name: &str) -> Self {
        let mut full_name = parent.full_name();
        full_name.push_str(JOIN);
        full_name.push_str(name);

        let tracker = parent.tracker.clone();
        let (trace_enabled, log_level) = tracker.get_entity_enables(&full_name);

        let entity = Self {
            name: String::from(name),
            parent: Some(parent.clone()),
            tag: parent.tracker.unique_tag(),
            log_level: AtomicUsize::new(log_level as usize),
            trace_enabled: AtomicBool::new(trace_enabled),
            tracker,
        };

        create!(entity);

        entity
    }

    /// Update the level at which log messages should be emitted by this entity
    pub fn set_log_level(&self, level: log::Level) {
        self.log_level.store(level as usize, Ordering::SeqCst);
    }

    /// Update the level at which binary trace messages should be emitted by
    /// this entity.
    pub fn set_trace_enabled(&self, enabled: bool) {
        self.trace_enabled.store(enabled, Ordering::SeqCst);
    }

    /// Returns the level at which log messages should be emitted by this
    /// entity.
    pub fn log_level(&self) -> log::Level {
        unsafe { std::mem::transmute(self.log_level.load(Ordering::Relaxed)) }
    }

    /// Returns the whether tracing is enabled or not for this entity.
    pub fn trace_enabled(&self) -> bool {
        self.trace_enabled.load(Ordering::Relaxed)
    }

    /// Returns the full hierarchical name of this entity
    pub fn full_name(&self) -> String {
        match &self.parent {
            Some(parent) => {
                let mut name = parent.full_name();
                name.push_str(JOIN);
                name.push_str(self.name.as_str());
                name
            }
            None => self.name.clone(),
        }
    }
}

impl Drop for Entity {
    fn drop(&mut self) {
        destroy!(self);
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entity")
            .field("name", &self.name)
            .field("parent", &self.parent)
            .field("tag", &self.tag)
            .finish()
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(parent) = &self.parent {
            parent.fmt(f).unwrap();
            write!(f, "{}{}", JOIN, self.name)
        } else {
            write!(f, "{}", self.name)
        }
    }
}

/// Create the top-level entity. This should be the only entity without a
/// parent.
pub fn toplevel(tracker: &Tracker, name: &str) -> Arc<Entity> {
    let (trace_enable, log_level) = tracker.get_entity_enables(name);
    let top = Arc::new(Entity {
        parent: None,
        name: String::from(name),
        tag: tracker.unique_tag(),
        log_level: AtomicUsize::new(log_level as usize),
        trace_enabled: AtomicBool::new(trace_enable),
        tracker: tracker.clone(),
    });
    create!(top);
    top
}
