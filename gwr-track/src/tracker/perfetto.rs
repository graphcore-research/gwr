// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
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
    /// IDs whose track descriptors have been written to the trace. Filtering
    /// can remove an ancestor while retaining a child, so parent links must
    /// only refer to IDs in this set.
    emitted_descriptors: RefCell<HashSet<Id>>,
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
            emitted_descriptors: RefCell::new(HashSet::new()),
        }
    }

    /// Omit the parent link when its descriptor was filtered out. Perfetto then
    /// treats the child as a root track instead of leaving it under an unknown
    /// parent, which can make the filtered trace appear empty in the UI.
    fn emitted_parent(&self, parent: Id) -> Option<Id> {
        self.emitted_descriptors
            .borrow()
            .contains(&parent)
            .then_some(parent)
    }
}

impl Track for PerfettoTracker {
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
                self.emitted_parent(created_by),
                name,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
            self.emitted_descriptors.borrow_mut().insert(id);
        }
    }

    fn create_monitor(&self, created_by: Id, id: Id, name: &str) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            let mut guard = self.trace_builder.borrow_mut();
            let trace_packet = guard.build_value_track_descriptor_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                self.emitted_parent(created_by),
                name,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
            self.emitted_descriptors.borrow_mut().insert(id);
        }
    }

    fn create_lane(&self, created_by: Id, id: Id, name: &str) {
        if self.is_entity_enabled(id, log::Level::Trace) {
            let mut guard = self.trace_builder.borrow_mut();
            let trace_packet = guard.build_activity_track_descriptor_trace_packet(
                *self.current_time_ns.borrow(),
                id,
                self.emitted_parent(created_by),
                name,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
            self.emitted_descriptors.borrow_mut().insert(id);
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
                self.emitted_parent(created_by),
                details,
            );
            let buf = guard.build_trace_to_bytes(vec![trace_packet]);
            self.writer.borrow_mut().write_all(&buf).unwrap();
            self.emitted_descriptors.borrow_mut().insert(id);
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
        self.writer.borrow_mut().flush().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use gwr_perfetto::protos::Trace;
    use gwr_perfetto::protos::trace_packet::Data;
    use prost::Message;

    use super::*;

    #[test]
    fn filtered_tracks_have_no_missing_parent_descriptors() {
        let mut manager = EntityManager::new(log::Level::Error);
        manager
            .add_entity_level_filter("^top::pe0($|::lane::)", log::Level::Trace)
            .unwrap();
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let tracker = PerfettoTracker::new(manager, Box::new(CapturedWriter(bytes.clone())));

        tracker.add_entity(Id(2), "top", None);
        tracker.create_entity(Id(0), Id(2), "top");
        tracker.add_entity(Id(3), "top::pe0", None);
        tracker.create_entity(Id(2), Id(3), "top::pe0");
        tracker.add_entity(Id(4), "top::pe0::lane::compute", None);
        tracker.create_lane(Id(3), Id(4), "top::pe0::lane::compute");
        tracker.begin_activity(Id(5), Id(4), "compute");
        tracker.end_activity(Id(5));

        let trace = Trace::decode(bytes.borrow().as_slice()).unwrap();
        let descriptors: Vec<_> = trace
            .packet
            .iter()
            .filter_map(|packet| match &packet.data {
                Some(Data::TrackDescriptor(descriptor)) => Some(descriptor),
                _ => None,
            })
            .collect();
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].uuid, Some(3));
        assert_eq!(descriptors[0].parent_uuid, None);
        assert_eq!(descriptors[1].uuid, Some(4));
        assert_eq!(descriptors[1].parent_uuid, Some(3));
        assert_eq!(
            trace
                .packet
                .iter()
                .filter(|packet| matches!(packet.data, Some(Data::TrackEvent(_))))
                .count(),
            2
        );
    }
    struct CapturedWriter(Rc<RefCell<Vec<u8>>>);

    impl Write for CapturedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
