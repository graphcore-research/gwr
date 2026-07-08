// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::entity::Capacity;
use crate::perfetto_trace_builder::PerfettoTraceBuilder;
use crate::tracker::EntityManager;
use crate::tracker::aka::AlternativeNames;
use crate::{Id, SharedWriter, Track, Writer};

/// A tracker that writes Perfetto binary data
pub struct PerfettoTracker {
    entity_manager: EntityManager,
    writer: SharedWriter,
    current_time_ns: RefCell<u64>,
    trace_builder: RefCell<PerfettoTraceBuilder>,
    group_memberships: RefCell<HashMap<Id, Id>>,
    activity_lanes: RefCell<HashMap<Id, Id>>,
}

impl PerfettoTracker {
    /// Create a new [`PerfettoTracker`] with an [`EntityManager`]
    pub fn new(entity_manager: EntityManager, writer: Writer) -> Self {
        Self {
            entity_manager,
            writer: Rc::new(RefCell::new(writer)),
            current_time_ns: RefCell::new(0),
            trace_builder: RefCell::new(PerfettoTraceBuilder::new()),
            group_memberships: RefCell::new(HashMap::new()),
            activity_lanes: RefCell::new(HashMap::new()),
        }
    }
}

impl Track for PerfettoTracker {
    fn unique_id(&self) -> Id {
        self.entity_manager.unique_id()
    }

    fn is_entity_enabled(&self, id: Id, level: log::Level) -> bool {
        self.entity_manager.is_log_enabled_at_level(id, level)
    }

    fn monitoring_window_size_for(&self, id: Id) -> Option<u64> {
        self.entity_manager.monitoring_window_size_for(id)
    }

    fn add_entity(&self, id: Id, entity_name: &str, alternative_names: AlternativeNames) {
        self.entity_manager
            .add_entity(id, entity_name, alternative_names);
    }

    fn enter(&self, id: Id, entered: Id) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            let guard = self.trace_builder.borrow_mut();
            let trace_packet = guard.build_enter_track_event_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                entered,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
        }
    }

    fn exit(&self, id: Id, exited: Id) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            let guard = self.trace_builder.borrow_mut();
            let trace_packet = guard.build_exit_track_event_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                exited,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
        }
    }

    fn value(&self, id: Id, value: f64) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            let guard = self.trace_builder.borrow_mut();
            let trace_packet = guard.build_value_track_event_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                value,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
        }
    }

    fn begin_activity(&self, activity: Id, lane: Id, name: &str) {
        if self.is_entity_enabled(lane, log::Level::Trace) {
            self.activity_lanes.borrow_mut().insert(activity, lane);
            let guard = self.trace_builder.borrow_mut();
            let correlation_id = self
                .group_memberships
                .borrow()
                .get(&activity)
                .map(|group_id| group_id.0);
            let trace_packet = guard.build_activity_begin_trace_packet(
                *self.current_time_ns.borrow(),
                lane,
                name,
                correlation_id,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
        }
    }

    fn add_to_group(&self, activity: Id, group_id: Id) {
        self.group_memberships
            .borrow_mut()
            .insert(activity, group_id);
    }

    fn remove_from_group(&self, activity: Id, group_id: Id) {
        let is_member = self.group_memberships.borrow().get(&activity) == Some(&group_id);
        if is_member {
            self.group_memberships.borrow_mut().remove(&activity);
        }
    }

    fn end_activity(&self, activity: Id) {
        if let Some(lane) = self.activity_lanes.borrow_mut().remove(&activity)
            && self.is_entity_enabled(lane, log::Level::Trace)
        {
            let guard = self.trace_builder.borrow_mut();
            let trace_packet =
                guard.build_activity_end_trace_packet(*self.current_time_ns.borrow(), lane);
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
        }
    }

    fn create_entity(&self, created_by: Id, id: Id, name: &str) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            let mut guard = self.trace_builder.borrow_mut();
            let trace_packet = guard.build_enter_exit_track_descriptor_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                created_by,
                name,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
        }
    }

    fn create_monitor(&self, created_by: Id, id: Id, name: &str) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            let mut guard = self.trace_builder.borrow_mut();
            let trace_packet = guard.build_value_track_descriptor_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                created_by,
                name,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
        }
    }

    fn create_lane(&self, created_by: Id, id: Id, name: &str) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            let mut guard = self.trace_builder.borrow_mut();
            let trace_packet = guard.build_activity_track_descriptor_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                created_by,
                name,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
        }
    }

    fn create_group(&self, _created_by: Id, _id: Id, _name: &str) {}

    fn create_object(
        &self,
        created_by: Id,
        id: Id,
        _size: usize,
        _units: &str,
        _req_type: u8,
        details: &str,
    ) {
        if self.is_entity_enabled(created_by, log::Level::Trace) {
            let mut guard = self.trace_builder.borrow_mut();
            let trace_packet = guard.build_enter_exit_track_descriptor_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                created_by,
                details,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
        }
    }

    fn capacity(&self, _id: Id, _capacity: Capacity) {
        // todo!()
    }

    fn destroy(&self, _destroyed_by: Id, _destroyed_obj: Id) {
        // todo!()
    }

    fn connect(&self, _connect_from: Id, _connect_to: Id) {
        // todo!()
    }

    fn log(&self, _msg_by: Id, _level: log::Level, _msg: std::fmt::Arguments) {
        // todo!()
    }

    fn time(&self, _set_by: Id, time_ns: f64) {
        *self.current_time_ns.borrow_mut() = time_ns as u64;
    }

    fn shutdown(&self) {
        // todo!()
    }
}
