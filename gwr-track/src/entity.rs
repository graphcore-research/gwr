// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A simulation entity.
//!
//! All parts of a model should contain an entity in order to maintain a
//! hierarchy of simulation entities. They contain a name and a unique ID
//! for tracing.

use std::fmt;
use std::rc::Rc;

use crate::tracker::aka::{Aka, get_alternative_names};
use crate::{Id, Tracker, create_id, destroy, trace};

/// A capacity value and its units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capacity {
    /// Capacity value.
    pub value: usize,

    /// Capacity units, for example "objects" or "bytes".
    pub units: String,
}

impl Capacity {
    /// Construct a new [`Capacity`].
    #[must_use]
    pub fn new(value: usize, units: impl Into<String>) -> Self {
        Self {
            value,
            units: units.into(),
        }
    }
}

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
        Self::new_with_renames(parent, name, None)
    }

    /// Create a new entity with a potential list of alternative names
    #[must_use]
    pub fn new_with_renames(parent: &Rc<Entity>, name: &str, aka: Option<&Aka>) -> Self {
        let alternative_names = get_alternative_names(aka, name);
        let mut full_name = parent.full_name();
        full_name.push_str(JOIN);
        full_name.push_str(name);

        let tracker = parent.tracker.clone();
        let id = create_id!(parent);
        tracker.add_entity(id, &full_name, alternative_names);

        let entity = Self {
            name: String::from(name),
            parent: Some(parent.clone()),
            id,
            tracker,
        };
        entity.track_create(parent.id, &full_name);

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

    /// Emit the capacity represented by this simulation entity.
    pub fn track_capacity(&self, value: usize, units: impl Into<String>) {
        self.tracker.capacity(self.id, Capacity::new(value, units));
    }

    /// Emit an enter event for an object.
    pub fn track_enter(&self, entered: Id) {
        self.tracker.enter(self.id, entered);
    }

    /// Emit an exit event for an object.
    pub fn track_exit(&self, exited: Id) {
        self.tracker.exit(self.id, exited);
    }

    /// Emit an object creation event.
    pub fn track_create_object(
        &self,
        created: Id,
        size: usize,
        units: &str,
        req_type: u8,
        details: &str,
    ) {
        self.tracker
            .create_object(self.id, created, size, units, req_type, details);
    }

    fn track_create(&self, created_by: Id, full_name: &str) {
        self.tracker.create_entity(created_by, self.id, full_name);
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
    top.track_create(crate::NO_ID, name);
    top
}

/// A monitor entity that is only allowed to emit value events.
pub struct EntityMonitor {
    /// The wrapped tracked entity.
    pub entity: Rc<Entity>,

    /// Unique simulation identifier used for bin/log messages.
    pub id: Id,

    /// Name of this monitor.
    pub name: String,
}

impl EntityMonitor {
    /// Create a new monitor entity.
    #[must_use]
    pub fn new(parent: &Rc<Entity>, name: &str) -> Self {
        let mut full_name = parent.full_name();
        full_name.push_str(JOIN);
        full_name.push_str(name);

        let id = create_id!(parent);
        parent.tracker.add_entity(id, &full_name, None);

        let monitor = Self {
            entity: parent.clone(),
            id,
            name: String::from(name),
        };

        monitor.track_create(parent.id, &full_name);

        monitor
    }

    fn track_create(&self, created_by: Id, full_name: &str) {
        self.entity
            .tracker
            .create_monitor(created_by, self.id, full_name);
    }

    /// Emit a value event for this monitor.
    pub fn track_value(&self, value: f64) {
        self.entity.tracker.value(self.entity.id, value);
    }
}

/// The `GetEntity` trait is used to provide access to an objects [Entity]
pub trait GetEntity {
    /// Return the [Entity]
    fn entity(&self) -> &Rc<Entity>;
}

impl GetEntity for EntityMonitor {
    fn entity(&self) -> &Rc<Entity> {
        &self.entity
    }
}
