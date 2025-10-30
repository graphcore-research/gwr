// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

pub use log;

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

    fn is_entity_enabled(&self, id: Id, level: log::Level) -> bool {
        self.entity_manager.is_log_enabled_at_level(id, level)
    }

    fn monitoring_window_size_for(&self, id: Id) -> Option<u64> {
        self.entity_manager.monitoring_window_size_for(id)
    }

    fn add_entity(&self, id: Id, entity_name: &str, alternative_names: AlternativeNames) {
        self.entity_manager
            .add_entity(id, entity_name, alternative_names);
    }

    fn enter(&self, id: Id, object: Id) {
        self.writer
            .borrow_mut()
            .write_all(format!("{id}: enter {object}\n").as_bytes())
            .unwrap();
    }

    fn exit(&self, id: Id, object: Id) {
        self.writer
            .borrow_mut()
            .write_all(format!("{id}: exit {object}\n").as_bytes())
            .unwrap();
    }

    fn value(&self, id: Id, value: f64) {
        self.writer
            .borrow_mut()
            .write_all(format!("{id}: value {value}\n").as_bytes())
            .unwrap();
    }

    fn create(&self, created_by: Id, id: Id, num_bytes: usize, req_type: i8, name: &str) {
        self.writer
            .borrow_mut()
            .write_all(
                format!("{created_by}: created {id}, {name}, {req_type}, {num_bytes} bytes\n")
                    .as_bytes(),
            )
            .unwrap();
    }

    fn destroy(&self, destroyed_by: Id, id: Id) {
        self.writer
            .borrow_mut()
            .write_all(format!("{destroyed_by}: destroyed {id}\n").as_bytes())
            .unwrap();
    }

    fn connect(&self, connect_from: Id, connect_to: Id) {
        self.writer
            .borrow_mut()
            .write_all(format!("{connect_from}: connect to {connect_to}\n").as_bytes())
            .unwrap();
    }

    fn log(&self, id: Id, level: log::Level, msg: std::fmt::Arguments) {
        self.writer
            .borrow_mut()
            .write_all(format!("{id}:{level}: {msg}\n").as_bytes())
            .unwrap();
    }

    fn time(&self, set_by: Id, time_ns: f64) {
        self.writer
            .borrow_mut()
            .write_all(format!("{set_by}: set time to {time_ns:.1}ns\n").as_bytes())
            .unwrap();
    }

    fn shutdown(&self) {
        self.writer.borrow_mut().flush().unwrap();
    }
}
