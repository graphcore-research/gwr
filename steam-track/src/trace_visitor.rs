// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! This module provides helper functions for dealing with Cap'n Proto binary
//! data.

use std::io::BufRead;

use capnp::serialize_packed;

use crate::steam_track_capnp::log::LogLevel;
use crate::{Tag, steam_track_capnp};

/// The `TraceVisitor` trait is the interface that allows a user to see all the
/// events as a binary file is processed.
///
/// Note that the tag will be [NO_ID](../../steam_track/constant.NO_ID.html) if
/// the user hasn't set it.
pub trait TraceVisitor {
    /// A log event.
    ///
    /// # Arguments
    ///
    /// * `tag` - The originator of this event.
    /// * `level` - The logging level of the message.
    /// * `message` - The string to emit with this event.
    fn log(&mut self, tag: Tag, level: log::Level, message: &str) {
        // Remove the unused variable warnings
        let _ = tag;
        let _ = level;
        let _ = message;
    }

    /// The creation of a unique tag.
    ///
    /// # Arguments
    ///
    /// * `created_by` - Tag of the entity causing the creation.
    /// * `tag` - The originator of this event.
    /// * `num_bytes` - Size of the created entity in bytes.
    /// * `req_type` - The type of request being traced (Read, Write, etc).
    /// * `name` - Name of the entity being created.
    fn create(&mut self, created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        // Remove the unused variable warnings
        let _ = created_by;
        let _ = tag;
        let _ = num_bytes;
        let _ = req_type;
        let _ = name;
    }

    /// The destruction of a unique tag.
    ///
    /// # Arguments
    ///
    /// * `destroyed_by` - Tag of the entity causing the destruction.
    /// * `tag` - The originator of this event.
    fn destroy(&mut self, destroyed_by: Tag, tag: Tag) {
        // Remove the unused variable warnings
        let _ = destroyed_by;
        let _ = tag;
    }

    /// One entity is connected to another.
    ///
    /// # Arguments
    ///
    /// * `connect_from` - Tag of the entity being connected from.
    /// * `connect_to` - Tag of the entity being connected to.
    fn connect(&mut self, connect_from: Tag, connect_to: Tag) {
        // Remove the unused variable warnings
        let _ = connect_from;
        let _ = connect_to;
    }

    /// A tag is entered (e.g. start of a function or block).
    ///
    /// # Arguments
    ///
    /// * `tag` - The originator of this event.
    /// * `entered` - The tag of the entity entering.
    fn enter(&mut self, tag: Tag, entered: Tag) {
        // Remove the unused variable warnings
        let _ = tag;
        let _ = entered;
    }

    /// A tag is exited (e.g. end of a function or block).
    ///
    /// # Arguments
    ///
    /// * `tag` - The originator of this event.
    /// * `exited` - The tag of the entity exiting.
    fn exit(&mut self, tag: Tag, exited: Tag) {
        // Remove the unused variable warnings
        let _ = tag;
        let _ = exited;
    }

    /// Advance simulation time.
    ///
    /// # Arguments
    ///
    /// * `tag` - The originator of this event.
    /// * `time_ns` - The new simulation time in `ns`.
    fn time(&mut self, tag: Tag, time_ns: f64) {
        // Remove the unused variable warnings
        let _ = tag;
        let _ = time_ns;
    }
}

/// Process a given Cap'n Proto file calling the visitor for each event found.
///
/// # Examples
///
/// A simple visitor that will count how many tags are used.
/// ```no_run
/// # use std::error::Error;
/// use std::fs::File;
/// use std::io::BufReader;
///
/// use steam_track::Tag;
/// use steam_track::trace_visitor::{TraceVisitor, process_capnp};
///
/// struct TagCounter {
///     pub count: usize,
/// }
///
/// impl TagCounter {
///     fn new() -> Self {
///         Self { count: 0 }
///     }
/// }
///
/// impl TraceVisitor for TagCounter {
///     fn create(
///         &mut self,
///         _created_by: Tag,
///         _tag: Tag,
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
/// let mut visitor = TagCounter::new();
/// process_capnp(&mut reader, &mut visitor);
/// println!("{} tags seen", visitor.count);
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
            .get_root::<steam_track_capnp::event::Reader>()
            .expect("failed to parse event");

        let tag = Tag(event.get_tag());
        match event.which() {
            Ok(steam_track_capnp::event::Which::Log(builder)) => {
                let access = builder.expect("failed to parse Log event");
                visitor.log(
                    tag,
                    to_log_level(access.get_level().expect("failed to parse Log event")),
                    access
                        .get_message()
                        .expect("failed to parse Message event")
                        .to_str()
                        .expect("Message is not a valid UTF-8 string"),
                );
            }
            Ok(steam_track_capnp::event::Which::Create(builder)) => {
                let access = builder.expect("failed to parse Entity event");
                visitor.create(
                    tag,
                    Tag(access.get_tag()),
                    access.get_num_bytes() as usize,
                    access.get_req_type(),
                    access
                        .get_name()
                        .expect("failed to parse Name")
                        .to_str()
                        .expect("Name is not a valid UTF-8 string"),
                );
            }
            Ok(steam_track_capnp::event::Which::Destroy(destroyed_by)) => {
                visitor.destroy(tag, Tag(destroyed_by));
            }
            Ok(steam_track_capnp::event::Which::Connect(connect_to)) => {
                visitor.connect(tag, Tag(connect_to));
            }
            Ok(steam_track_capnp::event::Which::Enter(entered)) => {
                visitor.enter(tag, Tag(entered));
            }
            Ok(steam_track_capnp::event::Which::Exit(exited)) => {
                visitor.exit(tag, Tag(exited));
            }
            Ok(steam_track_capnp::event::Which::Time(time)) => {
                visitor.time(tag, time);
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
