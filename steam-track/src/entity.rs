// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A simulation entity.
//!
//! All parts of a model should contain an entity in order to maintain a
//! hierarchy of simulation entities. They contain a name and a unique tag
//! for tracing.

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
        let tag = tracker.unique_tag();
        tracker.add_entity(tag, &full_name);

        let entity = Self {
            name: String::from(name),
            parent: Some(parent.clone()),
            tag,
            tracker,
        };

        create!(entity);

        entity
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
    let tag = tracker.unique_tag();
    tracker.add_entity(tag, name);
    let top = Arc::new(Entity {
        parent: None,
        name: String::from(name),
        tag,
        tracker: tracker.clone(),
    });
    create!(top);
    top
}
