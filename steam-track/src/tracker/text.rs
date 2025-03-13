// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::sync::{Arc, Mutex};

pub use log;

use crate::tracker::{EntityManager, Track};
use crate::{SharedWriter, Tag, Writer};

/// A simple text logger to output messages to a Writer.
pub struct TextTracker {
    entity_manager: Arc<EntityManager>,

    /// Writer to which all _log_ events will be written.
    writer: SharedWriter,
}

impl TextTracker {
    /// Create a new [`TextTracker`] with an [`EntityManager`].
    pub fn new(entity_manager: Arc<EntityManager>, writer: Writer) -> Self {
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

    fn get_entity_enables(&self, entity_name: &str) -> (bool, log::Level) {
        (
            self.entity_manager.trace_enabled_for(entity_name),
            self.entity_manager.log_level_for(entity_name),
        )
    }

    fn enter(&self, tag: Tag, object: Tag) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{}: enter {}\n", tag, object).as_bytes())
            .unwrap();
    }

    fn exit(&self, tag: Tag, object: Tag) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{}: exit {}\n", tag, object).as_bytes())
            .unwrap();
    }

    fn create(&self, created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        self.writer
            .lock()
            .unwrap()
            .write_all(
                format!(
                    "{}: created {}, {}, {}, {} bytes\n",
                    created_by, tag, name, req_type, num_bytes
                )
                .as_bytes(),
            )
            .unwrap();
    }

    fn destroy(&self, destroyed_by: Tag, tag: Tag) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{}: destroyed {}\n", destroyed_by, tag).as_bytes())
            .unwrap();
    }

    fn log(&self, tag: Tag, level: log::Level, msg: std::fmt::Arguments) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{}:{}: {}\n", tag, level, msg).as_bytes())
            .unwrap();
    }

    fn time(&self, set_by: Tag, time_ns: f64) {
        self.writer
            .lock()
            .unwrap()
            .write_all(format!("{}: set time to {:.1}ns\n", set_by, time_ns).as_bytes())
            .unwrap();
    }
}
