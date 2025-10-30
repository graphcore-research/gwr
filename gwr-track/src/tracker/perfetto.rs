// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use crate::perfetto_trace_builder::PerfettoTraceBuilder;
use crate::tracker::EntityManager;
use crate::tracker::aka::AlternativeNames;
use crate::tracker::types::ReqType;
use crate::{Id, SharedWriter, Track, Writer};

/// A tracker that writes Perfetto binary data
pub struct PerfettoTracker {
    entity_manager: EntityManager,
    writer: SharedWriter,
    current_time_ns: RefCell<u64>,
    trace_builder: RefCell<PerfettoTraceBuilder>,
}

impl PerfettoTracker {
    /// Create a new [`PerfettoTracker`] with an [`EntityManager`]
    pub fn new(entity_manager: EntityManager, writer: Writer) -> Self {
        Self {
            entity_manager,
            writer: Rc::new(RefCell::new(writer)),
            current_time_ns: RefCell::new(0),
            trace_builder: RefCell::new(PerfettoTraceBuilder::new()),
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
        let guard = self.trace_builder.borrow_mut();
        let trace_packet = guard.build_counter_track_event_trace_packet(
            *self.current_time_ns.borrow(),
            id,
            entered,
            1,
        );
        let buf = guard.build_trace_to_bytes(vec![trace_packet]);
        self.writer.borrow_mut().write_all(&buf).unwrap();
    }

    fn exit(&self, id: Id, exited: Id) {
        let guard = self.trace_builder.borrow_mut();
        let trace_packet = guard.build_counter_track_event_trace_packet(
            *self.current_time_ns.borrow(),
            id,
            exited,
            -1,
        );
        let buf = guard.build_trace_to_bytes(vec![trace_packet]);
        self.writer.borrow_mut().write_all(&buf).unwrap();
    }

    fn value(&self, id: Id, value: f64) {
        let guard = self.trace_builder.borrow_mut();
        let trace_packet =
            guard.build_value_track_event_trace_packet(*self.current_time_ns.borrow(), id, value);
        let buf = guard.build_trace_to_bytes(vec![trace_packet]);
        self.writer.borrow_mut().write_all(&buf).unwrap();
    }

    fn create(&self, created_by: Id, id: Id, _num_bytes: usize, req_type: i8, name: &str) {
        let mut guard = self.trace_builder.borrow_mut();
        let trace_packet = if req_type == ReqType::Value as i8 {
            guard.build_value_track_descriptor_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                created_by,
                name,
            )
        } else {
            guard.build_counter_track_descriptor_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                created_by,
                name,
            )
        };
        let buf = guard.build_trace_to_bytes(vec![trace_packet]);
        self.writer.borrow_mut().write_all(&buf).unwrap();
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
