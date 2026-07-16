// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use capnp::serialize_packed;

use crate::entity::Capacity;
use crate::gwr_track_capnp::event;
use crate::gwr_track_capnp::log::LogLevel;
use crate::tracker::aka::AlternativeNames;
use crate::tracker::{EntityManager, Track};
use crate::{Id, SharedWriter, Writer, gwr_track_capnp};

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
            writer: Rc::new(RefCell::new(writer)),
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
        F: FnOnce(gwr_track_capnp::event::Builder<'_>),
    {
        let mut builder = capnp::message::Builder::new_default();
        {
            let mut event = builder.init_root::<event::Builder>();
            event.set_id(id.0);

            // Call build method to populate the rest of the event
            build(event);
        }

        // Write out the event to the file
        let mut writer_ref = self.writer.borrow_mut();
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
            self.write_event(id, |mut event| {
                event.set_enter(object.0);
            });
        }
    }

    fn exit(&self, id: Id, object: Id) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            self.write_event(id, |mut event| {
                event.set_exit(object.0);
            });
        }
    }

    fn value(&self, id: Id, value: f64) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            self.write_event(id, |mut event| {
                event.set_value(value);
            });
        }
    }

    fn begin_activity(&self, activity: Id, lane: Id, name: &str) {
        if self.is_entity_enabled(lane, log::Level::Trace) {
            self.write_event(activity, |event| {
                let mut begin_activity = event.init_begin_activity();
                begin_activity.set_lane(lane.0);
                begin_activity.set_name(name);
            });
        }
    }

    fn add_to_group(&self, activity: Id, group_id: Id) {
        self.write_event(activity, |mut event| {
            event.set_add_to_group(group_id.0);
        });
    }

    fn remove_from_group(&self, activity: Id, group_id: Id) {
        self.write_event(activity, |mut event| {
            event.set_remove_from_group(group_id.0);
        });
    }

    fn end_activity(&self, activity: Id) {
        self.write_event(activity, |mut event| {
            event.set_end_activity(());
        });
    }

    fn create_entity(&self, created_by: Id, id: Id, name: &str) {
        // Don't filter this event as it could be required by a GUI
        self.write_event(created_by, |event| {
            let mut create = event.init_create();
            create.set_id(id.0);
            create.init_entity().set_name(name);
        });
    }

    fn create_monitor(&self, created_by: Id, id: Id, name: &str) {
        // Don't filter this event as it could be required by a GUI
        self.write_event(created_by, |event| {
            let mut create = event.init_create();
            create.set_id(id.0);
            create.init_monitor().set_name(name);
        });
    }

    fn create_lane(&self, created_by: Id, id: Id, name: &str) {
        // Don't filter this event as it could be required by a GUI
        self.write_event(created_by, |event| {
            let mut create = event.init_create();
            create.set_id(id.0);
            create.init_lane().set_name(name);
        });
    }

    fn create_group(&self, created_by: Id, id: Id, name: &str) {
        // Don't filter this event as it could be required by a GUI
        self.write_event(created_by, |event| {
            let mut create = event.init_create();
            create.set_id(id.0);
            create.init_group().set_name(name);
        });
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
            self.write_event(created_by, |event| {
                let mut create = event.init_create();
                create.set_id(id.0);
                let mut object = create.init_object();
                object.set_size(size as u64);
                object.set_units(units);
                object.set_type(req_type);
                object.set_details(details);
            });
        }
    }

    fn capacity(&self, id: Id, capacity: Capacity) {
        // Don't filter this event as it could be required by a GUI
        self.write_event(id, |event| {
            let mut event_capacity = event.init_capacity();
            event_capacity.set_value(capacity.value as u64);
            event_capacity.set_units(&capacity.units);
        });
    }

    fn destroy(&self, destroyed_by: Id, id: Id) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            self.write_event(destroyed_by, |mut event| {
                event.set_destroy(id.0);
            });
        }
    }

    fn connect(&self, connect_from: Id, connect_to: Id) {
        if self.is_entity_enabled(connect_from, log::Level::Trace)
            || self.is_entity_enabled(connect_to, log::Level::Trace)
        {
            self.write_event(connect_from, |mut event| {
                event.set_connect(connect_to.0);
            });
        }
    }

    fn log(&self, id: Id, level: log::Level, msg: std::fmt::Arguments) {
        if self.is_entity_enabled(id, level) {
            self.write_event(id, |event| {
                let mut log = event.init_log();
                let txt = format!("{msg}");
                log.set_message(&txt);
                log.set_level(to_capnp_log_level(level));
            });
        }
    }

    fn time(&self, set_by: Id, time_ns: f64) {
        self.write_event(set_by, |mut event| {
            event.set_time(time_ns);
        });
    }

    fn shutdown(&self) {
        self.writer.borrow_mut().flush().unwrap();
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
