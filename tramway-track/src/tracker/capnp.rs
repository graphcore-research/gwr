// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::sync::{Arc, Mutex};

use capnp::serialize_packed;

use crate::tracker::{EntityManager, Track};
use crate::tramway_track_capnp::event;
use crate::tramway_track_capnp::log::LogLevel;
use crate::{Id, SharedWriter, Writer, tramway_track_capnp};

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
    fn write_event<F>(&self, id: Id, build: F)
    where
        F: FnOnce(tramway_track_capnp::event::Builder<'_>),
    {
        let mut builder = capnp::message::Builder::new_default();
        {
            let mut event = builder.init_root::<event::Builder>();
            event.set_id(id.0);

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
    fn unique_id(&self) -> Id {
        self.entity_manager.unique_id()
    }

    fn is_entity_enabled(&self, id: Id, level: log::Level) -> bool {
        self.entity_manager.is_enabled(id, level)
    }

    fn add_entity(&self, id: Id, entity_name: &str) {
        self.entity_manager.add_entity(id, entity_name);
    }

    fn enter(&self, id: Id, object: Id) {
        self.write_event(id, |mut event| {
            event.set_enter(object.0);
        });
    }

    fn exit(&self, id: Id, object: Id) {
        self.write_event(id, |mut event| {
            event.set_exit(object.0);
        });
    }

    fn create(&self, created_by: Id, id: Id, num_bytes: usize, req_type: i8, name: &str) {
        self.write_event(created_by, |event| {
            let mut entity = event.init_create();
            entity.set_id(id.0);
            entity.set_num_bytes(num_bytes as u64);
            entity.set_req_type(req_type);
            entity.set_name(name);
        });
    }

    fn destroy(&self, destroyed_by: Id, id: Id) {
        self.write_event(destroyed_by, |mut event| {
            event.set_destroy(id.0);
        });
    }

    fn connect(&self, connect_from: Id, connect_to: Id) {
        self.write_event(connect_from, |mut event| {
            event.set_connect(connect_to.0);
        });
    }

    fn log(&self, id: Id, level: log::Level, msg: std::fmt::Arguments) {
        self.write_event(id, |event| {
            let mut log = event.init_log();
            let txt = format!("{msg}");
            log.set_message(&txt);
            log.set_level(to_capnp_log_level(level));
        });
    }

    fn time(&self, set_by: Id, time_ns: f64) {
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
