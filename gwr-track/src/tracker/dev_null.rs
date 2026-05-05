// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::str::FromStr;
use std::time::Duration;

use crate::Id;
use crate::entity::Capacity;
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
    fn capacity(&self, _id: Id, _capacity: Capacity) {}
    fn create_entity(&self, _created_by: Id, _id: Id, _name: &str) {}
    fn create_monitor(&self, _created_by: Id, _id: Id, _name: &str) {}
    fn create_object(
        &self,
        _created_by: Id,
        _id: Id,
        _size: usize,
        _units: &str,
        _req_type: u8,
        _details: &str,
    ) {
    }
    fn destroy(&self, _id: Id, _obj: Id) {}
    fn connect(&self, _connect_from: Id, _connect_to: Id) {}
    fn log(&self, _id: Id, _level: log::Level, _msg: std::fmt::Arguments) {}
    fn time(&self, _set_by: Id, _time: Duration) {}
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
