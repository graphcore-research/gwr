// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! This module provides helper functions for dealing with Cap'n Proto binary
//! data.

use std::io::BufRead;

use capnp::serialize_packed;

use crate::tramway_track_capnp::log::LogLevel;
use crate::{Id, tramway_track_capnp};

/// The `TraceVisitor` trait is the interface that allows a user to see all the
/// events as a binary file is processed.
///
/// Note that the ID will be [NO_ID](../../tramway_track/constant.NO_ID.html) if
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

    /// The creation of a unique ID.
    ///
    /// # Arguments
    ///
    /// * `created_by` - ID of the entity causing the creation.
    /// * `id` - The originator of this event.
    /// * `num_bytes` - Size of the created entity in bytes.
    /// * `req_type` - The type of request being traced (Read, Write, etc).
    /// * `name` - Name of the entity being created.
    fn create(&mut self, created_by: Id, id: Id, num_bytes: usize, req_type: i8, name: &str) {
        // Remove the unused variable warnings
        let _ = created_by;
        let _ = id;
        let _ = num_bytes;
        let _ = req_type;
        let _ = name;
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

    /// Advance simulation time.
    ///
    /// # Arguments
    ///
    /// * `id` - The originator of this event.
    /// * `time_ns` - The new simulation time in `ns`.
    fn time(&mut self, id: Id, time_ns: f64) {
        // Remove the unused variable warnings
        let _ = id;
        let _ = time_ns;
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
/// use tramway_track::Id;
/// use tramway_track::trace_visitor::{TraceVisitor, process_capnp};
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
///     fn create(
///         &mut self,
///         _created_by: Id,
///         _id: Id,
///         _num_bytes: usize,
///         _req_type: i8,
///         _name: &str,
///     ) {
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
            .get_root::<tramway_track_capnp::event::Reader>()
            .expect("failed to parse event");

        let id = Id(event.get_id());
        match event.which() {
            Ok(tramway_track_capnp::event::Which::Log(builder)) => {
                let access = builder.expect("failed to parse Log event");
                visitor.log(
                    id,
                    to_log_level(access.get_level().expect("failed to parse Log event")),
                    access
                        .get_message()
                        .expect("failed to parse Message event")
                        .to_str()
                        .expect("Message is not a valid UTF-8 string"),
                );
            }
            Ok(tramway_track_capnp::event::Which::Create(builder)) => {
                let access = builder.expect("failed to parse Entity event");
                visitor.create(
                    id,
                    Id(access.get_id()),
                    access.get_num_bytes() as usize,
                    access.get_req_type(),
                    access
                        .get_name()
                        .expect("failed to parse Name")
                        .to_str()
                        .expect("Name is not a valid UTF-8 string"),
                );
            }
            Ok(tramway_track_capnp::event::Which::Destroy(destroyed_by)) => {
                visitor.destroy(id, Id(destroyed_by));
            }
            Ok(tramway_track_capnp::event::Which::Connect(connect_to)) => {
                visitor.connect(id, Id(connect_to));
            }
            Ok(tramway_track_capnp::event::Which::Enter(entered)) => {
                visitor.enter(id, Id(entered));
            }
            Ok(tramway_track_capnp::event::Which::Exit(exited)) => {
                visitor.exit(id, Id(exited));
            }
            Ok(tramway_track_capnp::event::Which::Time(time)) => {
                visitor.time(id, time);
            }
            Err(e) => {
                panic!("failed to parse event ({e})");
            }
        }
    }
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
