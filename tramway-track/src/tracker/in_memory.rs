// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::HashMap;

use num_traits::FromPrimitive;

use super::types::ReqType;
use crate::Id;
use crate::tracker::{EntityManager, Track};

/// A [`Track`] event.
#[derive(Debug, Clone)]
pub struct EventCommon {
    /// The [`Id`] of the event originator.
    id: Id,

    /// The time at which the event occurred.
    time: f64,

    /// Any event-specific state.
    event: Event,
}

impl EventCommon {
    fn new(id: Id, time: f64, event: Event) -> Self {
        Self { id, time, event }
    }
}

#[derive(Debug, Clone)]
// TODO: Remove this once the tramway_spotter is using this structure
#[allow(dead_code)]
enum Event {
    Create { num_bytes: usize, req_type: i8 },
    Destroy,
    Connect { to: Id },
    Log { level: log::Level, text: String },
    Enter { entered: Id },
    Exit { exited: Id },
}

struct TrackedState {
    events: Vec<EventCommon>,
    id_to_num_bytes: HashMap<Id, usize>,
    id_to_req_type: HashMap<Id, i8>,
    name_to_id: HashMap<String, Id>,
}

impl TrackedState {
    /// Create a CapnProtoTracer
    fn new() -> Self {
        Self {
            events: Vec::with_capacity(INITIAL_CAPACITY),
            id_to_num_bytes: HashMap::with_capacity(INITIAL_CAPACITY),
            id_to_req_type: HashMap::with_capacity(INITIAL_CAPACITY),
            name_to_id: HashMap::with_capacity(INITIAL_CAPACITY),
        }
    }

    fn add_event(&mut self, event: EventCommon) {
        self.events.push(event);
    }

    fn add_id_to_num_bytes(&mut self, id: Id, num_bytes: usize) {
        self.id_to_num_bytes.insert(id, num_bytes);
    }

    fn add_id_to_req_type(&mut self, id: Id, req_type: i8) {
        self.id_to_req_type.insert(id, req_type);
    }

    fn add_name_to_id(&mut self, name: &str, id: Id) {
        self.name_to_id.insert(name.to_owned(), id);
    }

    fn id_for_name(&self, name: &str) -> Option<Id> {
        self.name_to_id.get(name).copied()
    }

    fn num_bytes_for_id(&self, id: Id) -> Option<usize> {
        self.id_to_num_bytes.get(&id).copied()
    }

    fn turnarounds_for_id(&self, id: Id) -> Option<i8> {
        self.id_to_req_type.get(&id).copied()
    }

    fn count_ingress(&self, id: Id) -> usize {
        self.events
            .iter()
            .filter(|e| e.id == id)
            .filter(|e| matches!(e.event, Event::Enter { entered: _ }))
            .count()
    }

    fn count_egress(&self, id: Id) -> usize {
        self.events
            .iter()
            .filter(|e| e.id == id)
            .filter(|e| matches!(e.event, Event::Exit { exited: _ }))
            .count()
    }

    fn bus_turnaround(&self, id: Id) -> usize {
        let mut first = true;
        let mut last_bus_req = ReqType::Read;
        let mut turnarounds = 0;
        for e in self.events.iter().filter(|e| e.id == id) {
            if let Event::Enter { entered } = e.event {
                let current_bus_req =
                    FromPrimitive::from_i8(self.turnarounds_for_id(entered).unwrap()).unwrap();
                if first {
                    first = false;
                } else {
                    match (last_bus_req, current_bus_req) {
                        (ReqType::Read, ReqType::Write)
                        | (ReqType::Read, ReqType::WriteNonPosted)
                        | (ReqType::Write, ReqType::Read)
                        | (ReqType::WriteNonPosted, ReqType::Read) => turnarounds += 1,
                        _ => (),
                    }
                }
                last_bus_req = current_bus_req;
            }
        }
        turnarounds
    }

    fn gbps_ingress(&self, id: Id) -> Option<f64> {
        let mut start_time_ns = None;
        let mut end_time_ns = None;
        let mut total_bytes = 0;
        for e in self.events.iter().filter(|e| e.id == id) {
            if let Event::Enter { entered } = e.event {
                if start_time_ns.is_none() {
                    start_time_ns = Some(e.time);
                }

                match self.num_bytes_for_id(entered) {
                    // Not possible to compute bandwidth as this packet bytes has not been recorded
                    None => return None,
                    Some(num_bytes) => total_bytes += num_bytes,
                }

                end_time_ns = Some(e.time);
            }
        }
        gbps(start_time_ns, end_time_ns, total_bytes)
    }

    fn gbps_egress(&self, id: Id) -> Option<f64> {
        let mut start_time_ns = None;
        let mut end_time_ns = None;
        let mut total_bytes = 0;
        for e in self.events.iter().filter(|e| e.id == id) {
            if let Event::Exit { exited } = e.event {
                if start_time_ns.is_none() {
                    start_time_ns = Some(e.time);
                }

                match self.num_bytes_for_id(exited) {
                    // Not possible to compute bandwidth as this packet bytes has not been recorded
                    None => return None,
                    Some(num_bytes) => total_bytes += num_bytes,
                }

                end_time_ns = Some(e.time);
            }
        }
        gbps(start_time_ns, end_time_ns, total_bytes)
    }

    fn gbps_through(&self, id: Id) -> Option<f64> {
        let mut start_time_ns = None;
        let mut end_time_ns = None;
        let mut total_bytes = 0;
        for e in self.events.iter().filter(|e| e.id == id) {
            match e.event {
                Event::Enter { entered: _ } => {
                    if start_time_ns.is_none() {
                        start_time_ns = Some(e.time);
                    }
                }
                Event::Exit { exited } => {
                    // Only count the number of bytes that have made it all the way through
                    match self.num_bytes_for_id(exited) {
                        // Not possible to compute bandwidth as this packet bytes has not been
                        // recorded
                        None => return None,
                        Some(num_bytes) => total_bytes += num_bytes,
                    }

                    end_time_ns = Some(e.time);
                }
                _ => {}
            }
        }
        gbps(start_time_ns, end_time_ns, total_bytes)
    }
}

fn gbps(start_time_ns: Option<f64>, end_time_ns: Option<f64>, total_bytes: usize) -> Option<f64> {
    if start_time_ns.is_none() && end_time_ns.is_none() {
        // Nothing seen at all
        Some(0.0)
    } else {
        // There should have been something seen
        let duration_ns = end_time_ns.unwrap() - start_time_ns.unwrap();
        if duration_ns > 0.0 {
            Some(8.0 * total_bytes as f64 / duration_ns)
        } else {
            Some(f64::INFINITY)
        }
    }
}

/// A tracer that writes Cap'n Proto binary data
pub struct InMemoryTracker {
    entity_manager: EntityManager,
    state: RefCell<TrackedState>,
}

const INITIAL_CAPACITY: usize = 10000;

impl InMemoryTracker {
    /// Create a new [`InMemoryTracker`] with an [`EntityManager`].
    pub fn new(entity_manager: EntityManager) -> Self {
        Self {
            entity_manager,
            state: RefCell::new(TrackedState::new()),
        }
    }

    fn add_event(&self, event: EventCommon) {
        let mut state_guard = self.state.borrow_mut();
        state_guard.add_event(event);
    }

    fn time(&self) -> f64 {
        self.entity_manager.time()
    }

    /// Get the [`Id`] for the specified simulation entity/object.
    pub fn id_for_name(&self, name: &str) -> Option<Id> {
        let state_guard = self.state.borrow_mut();
        state_guard.id_for_name(name)
    }

    /// Return the number of packets that exited the entity specified by `id`.
    pub fn count_egress(&self, id: Id) -> usize {
        let state_guard = self.state.borrow_mut();
        state_guard.count_egress(id)
    }

    /// Return the number of packets that entered the entity specified by `id`.
    pub fn ingress_count(&self, id: Id) -> usize {
        let state_guard = self.state.borrow_mut();
        state_guard.count_ingress(id)
    }

    /// Return the number of packets that exited the entity specified by `id`.
    pub fn bus_turnaround(&self, id: Id) -> usize {
        let state_guard = self.state.borrow_mut();
        state_guard.bus_turnaround(id)
    }

    /// Return the bandwidth through the specified entity.
    ///
    /// *Note*: returns None if the bandwidth cannot be calculated.
    pub fn gbps_through(&self, id: Id) -> Option<f64> {
        let state_guard = self.state.borrow_mut();
        state_guard.gbps_through(id)
    }

    /// Return the bandwidth at the ingress of the specified entity.
    ///
    /// *Note*: returns None if the bandwidth cannot be calculated.
    pub fn gbps_ingress(&self, id: Id) -> Option<f64> {
        let state_guard = self.state.borrow_mut();
        state_guard.gbps_ingress(id)
    }

    /// Return the bandwidth at the egress of the specified entity.
    ///
    /// *Note*: returns None if the bandwidth cannot be calculated.
    pub fn egress_gbps(&self, id: Id) -> Option<f64> {
        let state_guard = self.state.borrow_mut();
        state_guard.gbps_egress(id)
    }
}

/// Implementation each [`Track`] event
impl Track for InMemoryTracker {
    fn unique_id(&self) -> Id {
        self.entity_manager.unique_id()
    }

    fn is_entity_enabled(&self, id: Id, level: log::Level) -> bool {
        self.entity_manager.is_enabled(id, level)
    }

    fn add_entity(&self, id: Id, entity_name: &str) {
        self.entity_manager.add_entity(id, entity_name);
    }

    fn enter(&self, id: Id, object: Id) {
        let time = self.time();
        let enter = Event::Enter { entered: object };
        self.add_event(EventCommon::new(id, time, enter));
    }

    fn exit(&self, id: Id, object: Id) {
        let time = self.time();
        let exit = Event::Exit { exited: object };
        self.add_event(EventCommon::new(id, time, exit));
    }

    fn create(&self, _created_by: Id, id: Id, num_bytes: usize, req_type: i8, name: &str) {
        let time = self.time();
        let create = Event::Create {
            num_bytes,
            req_type,
        };
        let mut state_guard = self.state.borrow_mut();
        state_guard.add_event(EventCommon::new(id, time, create));
        state_guard.add_id_to_num_bytes(id, num_bytes);
        state_guard.add_id_to_req_type(id, req_type);
        state_guard.add_name_to_id(name, id);
    }

    /// Track when an entity with the given ID is destroyed.
    fn destroy(&self, _destroyed_by: Id, id: Id) {
        let time = self.time();
        let destroy = Event::Destroy;
        self.add_event(EventCommon::new(id, time, destroy));

        // TODO: Remove items from HashMaps to save memory?
    }

    /// Track when an entity is connected to another entity
    fn connect(&self, connect_from: Id, connect_to: Id) {
        let time = self.time();
        let connect = Event::Connect { to: connect_to };
        self.add_event(EventCommon::new(connect_from, time, connect));

        // TODO: Remove items from HashMaps to save memory?
    }

    /// Track a log message of the given level.
    fn log(&self, id: Id, level: log::Level, msg: std::fmt::Arguments) {
        let time = self.time();
        let log = Event::Log {
            level,
            text: format!("{msg}"),
        };
        self.add_event(EventCommon::new(id, time, log));
    }

    fn time(&self, _set_by: Id, time_ns: f64) {
        self.entity_manager.set_time(time_ns);
    }

    fn shutdown(&self) {
        // Do nothing
    }
}
