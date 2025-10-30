// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::str::FromStr;

use crate::Id;
use crate::tracker::Track;
use crate::tracker::aka::AlternativeNames;

/// A tracker that does nothing.
///
/// This can be useful for benchmarks that want to have minimum overheads.
pub struct DevNullTracker;

impl Track for DevNullTracker {
    fn unique_id(&self) -> Id {
        Id(0)
    }
    fn is_entity_enabled(&self, _id: Id, _level: log::Level) -> bool {
        false
    }
    fn monitoring_window_size_for(&self, _id: Id) -> Option<u64> {
        None
    }
    fn add_entity(&self, _id: Id, _entity_name: &str, _alternative_names: AlternativeNames) {}
    fn enter(&self, _id: Id, _obj: Id) {}
    fn exit(&self, _id: Id, _obj: Id) {}
    fn value(&self, _id: Id, _value: f64) {}
    fn create(&self, _id: Id, _obj: Id, _num_bytes: usize, _req_type: i8, _name: &str) {}
    fn destroy(&self, _id: Id, _obj: Id) {}
    fn connect(&self, _connect_from: Id, _connect_to: Id) {}
    fn log(&self, _id: Id, _level: log::Level, _msg: std::fmt::Arguments) {}
    fn time(&self, _set_by: Id, _time_ns: f64) {}
    fn shutdown(&self) {}
}

/// Take the command-line string and convert it to a Level
#[must_use]
pub fn str_to_level(lvl: &str) -> log::Level {
    match log::Level::from_str(lvl) {
        Ok(level) => level,
        Err(_) => panic!("Unable to parse level string '{lvl}'"),
    }
}
