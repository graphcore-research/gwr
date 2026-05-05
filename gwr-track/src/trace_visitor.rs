// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! This module provides helper functions for dealing with Cap'n Proto binary
//! data.

use std::io::BufRead;
use std::time::Duration;

use capnp::serialize_packed;

use crate::entity::Capacity;
use crate::gwr_track_capnp::log::LogLevel;
use crate::{Id, gwr_track_capnp};

/// The `TraceVisitor` trait is the interface that allows a user to see all the
/// events as a binary file is processed.
///
/// Note that the ID will be [NO_ID](../../gwr_track/constant.NO_ID.html) if
/// the user hasn't set it.
pub trait TraceVisitor {
    /// A log event.
    ///
    /// # Arguments
    ///
    /// * `id` - The originator of this event.
    /// * `level` - The logging level of the message.
    /// * `message` - The string to emit with this event.
    fn log(&mut self, id: Id, level: log::Level, message: &str) {
        // Remove the unused variable warnings
        let _ = id;
        let _ = level;
        let _ = message;
    }

    /// The creation of an entity.
    ///
    /// # Arguments
    ///
    /// * `created_by` - ID of the entity causing the creation.
    /// * `id` - The originator of this event.
    /// * `name` - Name of the entity being created.
    fn create_entity(&mut self, created_by: Id, id: Id, name: &str) {
        let _ = created_by;
        let _ = id;
        let _ = name;
    }

    /// The creation of a monitor.
    ///
    /// # Arguments
    ///
    /// * `created_by` - ID of the entity causing the creation.
    /// * `id` - The originator of this event.
    /// * `name` - Name of the monitor being created.
    fn create_monitor(&mut self, created_by: Id, id: Id, name: &str) {
        let _ = created_by;
        let _ = id;
        let _ = name;
    }

    /// The creation of an object.
    ///
    /// # Arguments
    ///
    /// * `created_by` - ID of the entity causing the creation.
    /// * `id` - The originator of this event.
    /// * `size` - Size of the created object.
    /// * `units` - Units for the created object size.
    /// * `req_type` - The type of request being traced (Read, Write, etc).
    /// * `details` - Additional detail for the created object.
    fn create_object(
        &mut self,
        created_by: Id,
        id: Id,
        size: usize,
        units: &str,
        req_type: u8,
        details: &str,
    ) {
        let _ = created_by;
        let _ = id;
        let _ = size;
        let _ = units;
        let _ = req_type;
        let _ = details;
    }

    /// The destruction of a unique ID.
    ///
    /// # Arguments
    ///
    /// * `destroyed_by` - ID of the entity causing the destruction.
    /// * `id` - The originator of this event.
    fn destroy(&mut self, destroyed_by: Id, id: Id) {
        // Remove the unused variable warnings
        let _ = destroyed_by;
        let _ = id;
    }

    /// One entity is connected to another.
    ///
    /// # Arguments
    ///
    /// * `connect_from` - ID of the entity being connected from.
    /// * `connect_to` - ID of the entity being connected to.
    fn connect(&mut self, connect_from: Id, connect_to: Id) {
        // Remove the unused variable warnings
        let _ = connect_from;
        let _ = connect_to;
    }

    /// A ID is entered (e.g. start of a function or block).
    ///
    /// # Arguments
    ///
    /// * `id` - The originator of this event.
    /// * `entered` - The ID of the entity entering.
    fn enter(&mut self, id: Id, entered: Id) {
        // Remove the unused variable warnings
        let _ = id;
        let _ = entered;
    }

    /// A ID is exited (e.g. end of a function or block).
    ///
    /// # Arguments
    ///
    /// * `id` - The originator of this event.
    /// * `exited` - The ID of the entity exiting.
    fn exit(&mut self, id: Id, exited: Id) {
        // Remove the unused variable warnings
        let _ = id;
        let _ = exited;
    }

    /// A value has been set by the specified ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The originator of this event.
    /// * `value` - The value.
    fn value(&mut self, id: Id, value: f64) {
        // Remove the unused variable warnings
        let _ = id;
        let _ = value;
    }

    /// A capacity has been set for the specified ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The originator of this event.
    /// * `capacity` - The entity capacity and its units.
    fn capacity(&mut self, id: Id, capacity: Capacity) {
        // Remove the unused variable warnings
        let _ = id;
        let _ = capacity;
    }

    /// Advance simulation time.
    ///
    /// # Arguments
    ///
    /// * `id` - The originator of this event.
    /// * `time` - The new simulation time.
    fn time(&mut self, id: Id, time: Duration) {
        // Remove the unused variable warnings
        let _ = id;
        let _ = time;
    }
}

/// Process a given Cap'n Proto file calling the visitor for each event found.
///
/// # Examples
///
/// A simple visitor that will count how many IDs are used.
/// ```no_run
/// # use std::error::Error;
/// use std::fs::File;
/// use std::io::BufReader;
///
/// use gwr_track::Id;
/// use gwr_track::trace_visitor::{TraceVisitor, process_capnp};
///
/// struct IdCounter {
///     pub count: usize,
/// }
///
/// impl IdCounter {
///     fn new() -> Self {
///         Self { count: 0 }
///     }
/// }
///
/// impl TraceVisitor for IdCounter {
///     fn create_entity(&mut self, _created_by: Id, _id: Id, _name: &str) {
///         self.count += 1;
///     }
/// }
///
/// # fn main() -> Result<(), Box<dyn Error>> {
/// let f = File::open("capnp.bin")?;
/// let mut reader = BufReader::new(f);
/// let mut visitor = IdCounter::new();
/// process_capnp(&mut reader, &mut visitor);
/// println!("{} IDs seen", visitor.count);
/// #
/// # Ok(())
/// # }
/// ```
pub fn process_capnp<R>(mut reader: R, visitor: &mut dyn TraceVisitor)
where
    R: BufRead,
{
    while let Ok(event_reader) =
        serialize_packed::read_message(&mut reader, ::capnp::message::ReaderOptions::new())
    {
        let event = event_reader
            .get_root::<gwr_track_capnp::event::Reader>()
            .expect("should be able to parse event");

        let id = Id(event.get_id());
        match event.which() {
            Ok(gwr_track_capnp::event::Which::Log(builder)) => handle_log(visitor, id, builder),
            Ok(gwr_track_capnp::event::Which::Create(builder)) => {
                handle_create(visitor, id, builder);
            }
            Ok(gwr_track_capnp::event::Which::Destroy(destroyed_by)) => {
                handle_destroy(visitor, id, destroyed_by);
            }
            Ok(gwr_track_capnp::event::Which::Connect(connect_to)) => {
                handle_connect(visitor, id, connect_to);
            }
            Ok(gwr_track_capnp::event::Which::Enter(entered)) => handle_enter(visitor, id, entered),
            Ok(gwr_track_capnp::event::Which::Exit(exited)) => handle_exit(visitor, id, exited),
            Ok(gwr_track_capnp::event::Which::Value(value)) => handle_value(visitor, id, value),
            Ok(gwr_track_capnp::event::Which::Capacity(capacity)) => {
                handle_capacity(visitor, id, capacity);
            }
            Ok(gwr_track_capnp::event::Which::Time(time)) => handle_time(visitor, id, time),
            Err(e) => {
                panic!("should be able to parse event ({e})");
            }
        }
    }
}

fn handle_log(
    visitor: &mut dyn TraceVisitor,
    id: Id,
    builder: capnp::Result<gwr_track_capnp::log::Reader<'_>>,
) {
    let access = builder.expect("should be able to parse Log event");
    visitor.log(
        id,
        to_log_level(
            access
                .get_level()
                .expect("should be able to parse Log level"),
        ),
        access
            .get_message()
            .expect("should be able to parse Log message")
            .to_str()
            .expect("Log message should be valid UTF-8 string"),
    );
}

fn handle_create(
    visitor: &mut dyn TraceVisitor,
    id: Id,
    builder: capnp::Result<gwr_track_capnp::create::Reader<'_>>,
) {
    let access = builder.expect("should be able to parse Create event");
    let created_id = Id(access.get_id());
    match access.which() {
        Ok(gwr_track_capnp::create::Which::Entity(entity)) => {
            let entity = entity.expect("should be able to parse Create Entity");
            visitor.create_entity(
                id,
                created_id,
                entity
                    .get_name()
                    .expect("should be able to parse Entity name")
                    .to_str()
                    .expect("Create Entity name should be valid UTF-8 string"),
            );
        }
        Ok(gwr_track_capnp::create::Which::Monitor(monitor)) => {
            let monitor = monitor.expect("should be able to parse Create Monitor");
            visitor.create_monitor(
                id,
                created_id,
                monitor
                    .get_name()
                    .expect("should be able to parse Monitor name")
                    .to_str()
                    .expect("Create Monitor name should be valid UTF-8 string"),
            );
        }
        Ok(gwr_track_capnp::create::Which::Object(object)) => {
            let object = object.expect("should be able to parse Create Object");
            visitor.create_object(
                id,
                created_id,
                object.get_size() as usize,
                object
                    .get_units()
                    .expect("should be able to parse Object units")
                    .to_str()
                    .expect("Create Object units should be valid UTF-8 string"),
                object.get_type(),
                object
                    .get_details()
                    .expect("should be able to parse Object details")
                    .to_str()
                    .expect("Create Object details should be valid UTF-8 string"),
            );
        }
        Err(e) => panic!("should be able to parse create event ({e})"),
    }
}

fn handle_destroy(visitor: &mut dyn TraceVisitor, id: Id, destroyed_by: u64) {
    visitor.destroy(id, Id(destroyed_by));
}

fn handle_connect(visitor: &mut dyn TraceVisitor, id: Id, connect_to: u64) {
    visitor.connect(id, Id(connect_to));
}

fn handle_enter(visitor: &mut dyn TraceVisitor, id: Id, entered: u64) {
    visitor.enter(id, Id(entered));
}

fn handle_exit(visitor: &mut dyn TraceVisitor, id: Id, exited: u64) {
    visitor.exit(id, Id(exited));
}

fn handle_value(visitor: &mut dyn TraceVisitor, id: Id, value: f64) {
    visitor.value(id, value);
}

fn handle_capacity(
    visitor: &mut dyn TraceVisitor,
    id: Id,
    capacity: capnp::Result<gwr_track_capnp::capacity::Reader<'_>>,
) {
    let capacity = capacity.expect("should be able to parse Capacity event");
    visitor.capacity(
        id,
        Capacity::new(
            capacity.get_value() as usize,
            capacity
                .get_units()
                .expect("should be able to parse Capacity units")
                .to_str()
                .expect("Capacity units should be valid UTF-8 string"),
        ),
    );
}

fn handle_time(
    visitor: &mut dyn TraceVisitor,
    id: Id,
    time: capnp::Result<gwr_track_capnp::duration::Reader<'_>>,
) {
    let time = time.expect("should be able to parse Duration");
    visitor.time(
        id,
        Duration::new(time.get_seconds().into(), time.get_nanosecs()),
    );
}

fn to_log_level(level: LogLevel) -> log::Level {
    match level {
        LogLevel::Error => log::Level::Error,
        LogLevel::Warn => log::Level::Warn,
        LogLevel::Info => log::Level::Info,
        LogLevel::Debug => log::Level::Debug,
        LogLevel::Trace => log::Level::Trace,
    }
}
