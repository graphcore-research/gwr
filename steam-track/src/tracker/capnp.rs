// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::sync::{Arc, Mutex};

use capnp::serialize_packed;

use crate::steam_track_capnp;
use crate::steam_track_capnp::event;
use crate::steam_track_capnp::log::LogLevel;
use crate::tracker::{EntityManager, Track};
use crate::{SharedWriter, Tag, Writer};

/// A tracker that writes Cap'n Proto binary data
pub struct CapnProtoTracker {
    entity_manager: EntityManager,
    writer: SharedWriter,
}

impl CapnProtoTracker {
    /// Create a new [`CapnProtoTracker`] with an [`EntityManager`]
    pub fn new(entity_manager: EntityManager, writer: Writer) -> Self {
        Self {
            entity_manager,
            writer: Arc::new(Mutex::new(writer)),
        }
    }

    /// Helper function to create a _trace_ event
    ///
    /// # Arguments
    ///
    /// * `event_loc` - A [EventLocation](struct.EventLocation.html) giving
    ///   details of the location
    /// * `lvl` - The logging level which is used to filter events
    /// * `build` - The event builder function
    fn write_event<F>(&self, tag: Tag, build: F)
    where
        F: FnOnce(steam_track_capnp::event::Builder<'_>),
    {
        let mut builder = capnp::message::Builder::new_default();
        {
            let mut event = builder.init_root::<event::Builder>();
            event.set_tag(tag.0);

            // Call build method to populate the rest of the event
            build(event);
        }

        // Write out the event to the file
        let mut writer_ref = self.writer.lock().unwrap();
        serialize_packed::write_message(&mut *writer_ref, &builder).unwrap();
    }
}

/// Implementation each [`Track`] event
///
/// There is a function to emit each Cap'n Proto event structure. These
/// functions call the helper function
/// [`write_event`](crate::tracker::capnp::CapnProtoTracker), passing in a
/// function that is used to populate the event body.
impl Track for CapnProtoTracker {
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
        self.write_event(tag, |mut event| {
            event.set_enter(object.0);
        });
    }

    fn exit(&self, tag: Tag, object: Tag) {
        self.write_event(tag, |mut event| {
            event.set_exit(object.0);
        });
    }

    fn create(&self, created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        self.write_event(created_by, |event| {
            let mut entity = event.init_create();
            entity.set_tag(tag.0);
            entity.set_num_bytes(num_bytes as u64);
            entity.set_req_type(req_type);
            entity.set_name(name);
        });
    }

    fn destroy(&self, destroyed_by: Tag, tag: Tag) {
        self.write_event(destroyed_by, |mut event| {
            event.set_destroy(tag.0);
        });
    }

    fn connect(&self, connect_from: Tag, connect_to: Tag) {
        self.write_event(connect_from, |mut event| {
            event.set_connect(connect_to.0);
        });
    }

    fn log(&self, tag: Tag, level: log::Level, msg: std::fmt::Arguments) {
        self.write_event(tag, |event| {
            let mut log = event.init_log();
            let txt = format!("{msg}");
            log.set_message(&txt);
            log.set_level(to_capnp_log_level(level));
        });
    }

    fn time(&self, set_by: Tag, time_ns: f64) {
        self.write_event(set_by, |mut event| {
            event.set_time(time_ns);
        });
    }

    fn shutdown(&self) {
        self.writer.lock().unwrap().flush().unwrap();
    }
}

fn to_capnp_log_level(level: log::Level) -> LogLevel {
    match level {
        log::Level::Error => LogLevel::Error,
        log::Level::Warn => LogLevel::Warn,
        log::Level::Info => LogLevel::Info,
        log::Level::Debug => LogLevel::Debug,
        log::Level::Trace => LogLevel::Trace,
    }
}
