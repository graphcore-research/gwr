// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

#[doc(hidden)]
pub use log;

use crate::entity::Capacity;
use crate::tracker::aka::AlternativeNames;
use crate::tracker::{EntityManager, Track};
use crate::{Id, SharedWriter, Writer};

/// A simple text logger to output messages to a Writer.
pub struct TextTracker {
    entity_manager: EntityManager,

    /// Writer to which all _log_ events will be written.
    writer: SharedWriter,
}

impl TextTracker {
    /// Create a new [`TextTracker`] with an [`EntityManager`].
    pub fn new(entity_manager: EntityManager, writer: Writer) -> Self {
        Self {
            entity_manager,
            writer: Rc::new(RefCell::new(writer)),
        }
    }
}

/// Implementation for each [`Track`] event
impl Track for TextTracker {
    fn unique_id(&self) -> Id {
        self.entity_manager.unique_id()
    }

    fn enabled_level(&self, id: Id) -> log::Level {
        self.entity_manager.enabled_level(id)
    }

    fn monitoring_window_size_for(&self, id: Id) -> Option<u64> {
        self.entity_manager.monitoring_window_size_for(id)
    }

    fn add_entity(
        &self,
        id: Id,
        entity_name: &str,
        alternative_names: AlternativeNames,
    ) -> log::Level {
        self.entity_manager
            .add_entity(id, entity_name, alternative_names)
    }

    fn enter(&self, id: Id, object: Id) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{id}: enter {object}\n").as_bytes())
                .unwrap();
        }
    }

    fn exit(&self, id: Id, object: Id) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{id}: exit {object}\n").as_bytes())
                .unwrap();
        }
    }

    fn value(&self, id: Id, value: f64) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{id}: value {value}\n").as_bytes())
                .unwrap();
        }
    }

    fn add_to_group(&self, activity: Id, group_id: Id) {
        if self.is_entity_enabled(activity, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{activity}: added to group {group_id}\n").as_bytes())
                .unwrap();
        }
    }

    fn remove_from_group(&self, activity: Id, group_id: Id) {
        if self.is_entity_enabled(activity, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{activity}: removed from group {group_id}\n").as_bytes())
                .unwrap();
        }
    }

    fn begin_activity(&self, activity: Id, lane: Id, name: &str) {
        if self.is_entity_enabled(lane, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{activity}: activity begin {name} on lane {lane}\n").as_bytes())
                .unwrap();
        }
    }

    fn end_activity(&self, activity: Id) {
        if self.is_entity_enabled(activity, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{activity}: activity end\n").as_bytes())
                .unwrap();
        }
    }

    fn create_entity(&self, created_by: Id, id: Id, name: &str) {
        if self.is_entity_enabled(created_by, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{created_by}: created entity {id}, {name}\n").as_bytes())
                .unwrap();
        }
    }

    fn create_monitor(&self, created_by: Id, id: Id, name: &str) {
        if self.is_entity_enabled(created_by, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{created_by}: created monitor {id}, {name}\n").as_bytes())
                .unwrap();
        }
    }

    fn create_lane(&self, created_by: Id, id: Id, name: &str) {
        if self.is_entity_enabled(created_by, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{created_by}: created lane {id}, {name}\n").as_bytes())
                .unwrap();
        }
    }

    fn create_group(&self, created_by: Id, id: Id, name: &str) {
        if self.is_entity_enabled(created_by, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{created_by}: created group {id}, {name}\n").as_bytes())
                .unwrap();
        }
    }

    fn create_object(
        &self,
        created_by: Id,
        id: Id,
        size: usize,
        units: &str,
        req_type: u8,
        details: &str,
    ) {
        if self.is_entity_enabled(created_by, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(
                    format!(
                        "{created_by}: created object {id}, {req_type}, {size}, {units}, {details}\n"
                    )
                    .as_bytes(),
                )
                .unwrap();
        }
    }

    fn capacity(&self, id: Id, capacity: Capacity) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(
                    format!("{id}: capacity {} {}\n", capacity.value, capacity.units).as_bytes(),
                )
                .unwrap();
        }
    }

    fn destroy(&self, destroyed_by: Id, id: Id) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{destroyed_by}: destroyed {id}\n").as_bytes())
                .unwrap();
        }
    }

    fn connect(&self, connect_from: Id, connect_to: Id) {
        if self.is_entity_enabled(connect_from, log::Level::Trace)
            || self.is_entity_enabled(connect_to, log::Level::Trace)
        {
            self.writer
                .borrow_mut()
                .write_all(format!("{connect_from}: connect to {connect_to}\n").as_bytes())
                .unwrap();
        }
    }

    fn log(&self, id: Id, level: log::Level, msg: std::fmt::Arguments) {
        if self.is_entity_enabled(id, level) {
            self.writer
                .borrow_mut()
                .write_all(format!("{id}:{level}: {msg}\n").as_bytes())
                .unwrap();
        }
    }

    fn time(&self, set_by: Id, time_ns: f64) {
        if self.is_entity_enabled(set_by, log::Level::Trace) {
            self.writer
                .borrow_mut()
                .write_all(format!("{set_by}: set time to {time_ns:.1}ns\n").as_bytes())
                .unwrap();
        }
    }

    fn shutdown(&self) {
        self.writer.borrow_mut().flush().unwrap();
    }
}
