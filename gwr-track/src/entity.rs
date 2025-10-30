// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A simulation entity.
//!
//! All parts of a model should contain an entity in order to maintain a
//! hierarchy of simulation entities. They contain a name and a unique ID
//! for tracing.

use std::fmt;
use std::rc::Rc;

use crate::tracker::aka::{Aka, get_alternative_names};
use crate::{Id, Tracker, create, destroy, trace};

/// A simulation entity
///
/// An entity is a part of a hierarchical simulation in which it must have a
/// parent. The simulation top-level should be created using `toplevel("name")`.
///
/// The entity is used when logging so that its unique ID can be emitted and
/// it can determine which messages are emitted to both the binary and textual
/// outputs.
pub struct Entity {
    /// Name of this entity.
    pub name: String,

    /// Optional parent entity (only the top-level should be None).
    pub parent: Option<Rc<Entity>>,

    /// Unique simulation identifier used for bin/log messages.
    pub id: Id,

    /// [`Tracker`] used to handle trace/log events.
    pub tracker: Tracker,
}

static JOIN: &str = "::";

impl Entity {
    /// Create a new entity.
    #[must_use]
    pub fn new(parent: &Rc<Entity>, name: &str) -> Self {
        Self::new_with_renames_and_optional_create(parent, name, None, true)
    }

    /// Create a new entity with a potential list of alternative names
    #[must_use]
    pub fn new_with_renames(parent: &Rc<Entity>, name: &str, aka: Option<&Aka>) -> Self {
        Self::new_with_renames_and_optional_create(parent, name, aka, true)
    }

    /// Create a new entity but let the user do their own call to the
    /// `create!()` macro.
    #[must_use]
    pub fn new_without_create(parent: &Rc<Entity>, name: &str) -> Self {
        Self::new_with_renames_and_optional_create(parent, name, None, false)
    }

    /// Create a new entity with a potential list of alternative names
    #[must_use]
    pub fn new_with_renames_and_optional_create(
        parent: &Rc<Entity>,
        name: &str,
        aka: Option<&Aka>,
        do_create: bool,
    ) -> Self {
        let alternative_names = get_alternative_names(aka, name);
        let mut full_name = parent.full_name();
        full_name.push_str(JOIN);
        full_name.push_str(name);

        let tracker = parent.tracker.clone();
        let id = tracker.unique_id();
        tracker.add_entity(id, &full_name, alternative_names);

        let entity = Self {
            name: String::from(name),
            parent: Some(parent.clone()),
            id,
            tracker,
        };

        if do_create {
            create!(entity);
        }

        if let Some(alternative_names) = alternative_names {
            for name in alternative_names {
                trace!(entity ; "aka {name}");
            }
        }

        entity
    }

    /// Returns the full hierarchical name of this entity
    #[must_use]
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
            .field("id", &self.id)
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
pub fn toplevel(tracker: &Tracker, name: &str) -> Rc<Entity> {
    let id = tracker.unique_id();
    tracker.add_entity(id, name, None);
    let top = Rc::new(Entity {
        parent: None,
        name: String::from(name),
        id,
        tracker: tracker.clone(),
    });
    create!(top);
    top
}
