// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::sync::{Arc, Mutex};

pub use log;

use crate::tracker::{EntityManager, Track};
use crate::{SharedWriter, Tag, Writer};

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
            writer: Arc::new(Mutex::new(writer)),
        }
    }
}

/// Implementation for each [`Track`] event
impl Track for TextTracker {
    fn unique_tag(&self) -> Tag {
        self.entity_manager.unique_tag()
    }

    fn is_entity_enabled(&self, tag: Tag, level: log::Level) -> bool {
        self.entity_manager.is_enabled(tag, level)
    }

    fn add_entity(&self, tag: Tag, entity_name: &str) {
        self.entity_manager.add_entity(tag, entity_name);
    }

    fn enter(&self, tag: Tag, object: Tag) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{tag}: enter {object}\n").as_bytes())
            .unwrap();
    }

    fn exit(&self, tag: Tag, object: Tag) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{tag}: exit {object}\n").as_bytes())
            .unwrap();
    }

    fn create(&self, created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        self.writer
            .lock()
            .unwrap()
            .write_all(
                format!("{created_by}: created {tag}, {name}, {req_type}, {num_bytes} bytes\n")
                    .as_bytes(),
            )
            .unwrap();
    }

    fn destroy(&self, destroyed_by: Tag, tag: Tag) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{destroyed_by}: destroyed {tag}\n").as_bytes())
            .unwrap();
    }

    fn connect(&self, connect_from: Tag, connect_to: Tag) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{connect_from}: connect to {connect_to}\n").as_bytes())
            .unwrap();
    }

    fn log(&self, tag: Tag, level: log::Level, msg: std::fmt::Arguments) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{tag}:{level}: {msg}\n").as_bytes())
            .unwrap();
    }

    fn time(&self, set_by: Tag, time_ns: f64) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{set_by}: set time to {time_ns:.1}ns\n").as_bytes())
            .unwrap();
    }

    fn shutdown(&self) {
        self.writer.lock().unwrap().flush().unwrap();
    }
}
