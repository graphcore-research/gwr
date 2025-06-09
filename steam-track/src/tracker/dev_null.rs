// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::str::FromStr;

use crate::Tag;
use crate::tracker::Track;

/// A tracker that does nothing.
///
/// This can be useful for benchmarks that want to have minimum overheads.
pub struct DevNullTracker;

impl Track for DevNullTracker {
    fn unique_tag(&self) -> Tag {
        Tag(0)
    }

    fn is_entity_enabled(&self, _tag: Tag, _level: log::Level) -> bool {
        false
    }
    fn add_entity(&self, _tag: Tag, _entity_name: &str) {}
    fn enter(&self, _tag: Tag, _obj: Tag) {}
    fn exit(&self, _tag: Tag, _obj: Tag) {}
    fn create(&self, _tag: Tag, _obj: Tag, _num_bytes: usize, _req_type: i8, _name: &str) {}
    fn destroy(&self, _tag: Tag, _obj: Tag) {}
    fn log(&self, _tag: Tag, _level: log::Level, _msg: std::fmt::Arguments) {}
    fn time(&self, _set_by: Tag, _time_ns: f64) {}
    fn shutdown(&self) {}
}

/// Take the command-line string and convert it to a Level
pub fn str_to_level(lvl: &str) -> log::Level {
    match log::Level::from_str(lvl) {
        Ok(level) => level,
        Err(_) => panic!("Unable to parse level string '{}'", lvl),
    }
}
