// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use num_traits::FromPrimitive;

use super::types::ReqType;
use crate::Tag;
use crate::tracker::{EntityManager, Track};

/// A [`Track`] event.
#[derive(Debug, Clone)]
pub struct EventCommon {
    /// The [`Tag`](crate::Tag) of the event originator.
    tag: Tag,

    /// The time at which the event occurred.
    time: f64,

    /// Any event-specific state.
    event: Event,
}

impl EventCommon {
    fn new(tag: Tag, time: f64, event: Event) -> Self {
        Self { tag, time, event }
    }
}

#[derive(Debug, Clone)]
// TODO: Remove this once the steam_spotter is using this structure
#[allow(dead_code)]
enum Event {
    Create { num_bytes: usize, req_type: i8 },
    Destroy,
    Log { level: log::Level, text: String },
    Enter { entered: Tag },
    Exit { exited: Tag },
}

struct TrackedState {
    events: Vec<EventCommon>,
    tag_to_num_bytes: HashMap<Tag, usize>,
    tag_to_req_type: HashMap<Tag, i8>,
    name_to_tag: HashMap<String, Tag>,
}

impl TrackedState {
    /// Create a CapnProtoTracer
    fn new() -> Self {
        Self {
            events: Vec::with_capacity(INITIAL_CAPACITY),
            tag_to_num_bytes: HashMap::with_capacity(INITIAL_CAPACITY),
            tag_to_req_type: HashMap::with_capacity(INITIAL_CAPACITY),
            name_to_tag: HashMap::with_capacity(INITIAL_CAPACITY),
        }
    }

    fn add_event(&mut self, event: EventCommon) {
        self.events.push(event);
    }

    fn add_tag_to_num_bytes(&mut self, tag: Tag, num_bytes: usize) {
        self.tag_to_num_bytes.insert(tag, num_bytes);
    }

    fn add_tag_to_req_type(&mut self, tag: Tag, req_type: i8) {
        self.tag_to_req_type.insert(tag, req_type);
    }

    fn add_name_to_tag(&mut self, name: &str, tag: Tag) {
        self.name_to_tag.insert(name.to_owned(), tag);
    }

    fn tag_for_name(&self, name: &str) -> Option<Tag> {
        self.name_to_tag.get(name).copied()
    }

    fn num_bytes_for_tag(&self, tag: Tag) -> Option<usize> {
        self.tag_to_num_bytes.get(&tag).copied()
    }

    fn turnarounds_for_tag(&self, tag: Tag) -> Option<i8> {
        self.tag_to_req_type.get(&tag).copied()
    }

    fn count_ingress(&self, tag: Tag) -> usize {
        self.events
            .iter()
            .filter(|e| e.tag == tag)
            .filter(|e| matches!(e.event, Event::Enter { entered: _ }))
            .count()
    }

    fn count_egress(&self, tag: Tag) -> usize {
        self.events
            .iter()
            .filter(|e| e.tag == tag)
            .filter(|e| matches!(e.event, Event::Exit { exited: _ }))
            .count()
    }

    fn bus_turnaround(&self, tag: Tag) -> usize {
        let mut first = true;
        let mut last_bus_req = ReqType::Read;
        let mut turnarounds = 0;
        for e in self.events.iter().filter(|e| e.tag == tag) {
            if let Event::Enter { entered } = e.event {
                let current_bus_req =
                    FromPrimitive::from_i8(self.turnarounds_for_tag(entered).unwrap()).unwrap();
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

    fn gbps_ingress(&self, tag: Tag) -> Option<f64> {
        let mut start_time_ns = None;
        let mut end_time_ns = None;
        let mut total_bytes = 0;
        for e in self.events.iter().filter(|e| e.tag == tag) {
            if let Event::Enter { entered } = e.event {
                if start_time_ns.is_none() {
                    start_time_ns = Some(e.time);
                }

                match self.num_bytes_for_tag(entered) {
                    // Not possible to compute bandwidth as this packet bytes has not been recorded
                    None => return None,
                    Some(num_bytes) => total_bytes += num_bytes,
                }

                end_time_ns = Some(e.time);
            }
        }
        gbps(start_time_ns, end_time_ns, total_bytes)
    }

    fn gbps_egress(&self, tag: Tag) -> Option<f64> {
        let mut start_time_ns = None;
        let mut end_time_ns = None;
        let mut total_bytes = 0;
        for e in self.events.iter().filter(|e| e.tag == tag) {
            if let Event::Exit { exited } = e.event {
                if start_time_ns.is_none() {
                    start_time_ns = Some(e.time);
                }

                match self.num_bytes_for_tag(exited) {
                    // Not possible to compute bandwidth as this packet bytes has not been recorded
                    None => return None,
                    Some(num_bytes) => total_bytes += num_bytes,
                }

                end_time_ns = Some(e.time);
            }
        }
        gbps(start_time_ns, end_time_ns, total_bytes)
    }

    fn gbps_through(&self, tag: Tag) -> Option<f64> {
        let mut start_time_ns = None;
        let mut end_time_ns = None;
        let mut total_bytes = 0;
        for e in self.events.iter().filter(|e| e.tag == tag) {
            match e.event {
                Event::Enter { entered: _ } => {
                    if start_time_ns.is_none() {
                        start_time_ns = Some(e.time);
                    }
                }
                Event::Exit { exited } => {
                    // Only count the number of bytes that have made it all the way through
                    match self.num_bytes_for_tag(exited) {
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
    entity_manager: Arc<EntityManager>,
    state: Mutex<TrackedState>,
}

const INITIAL_CAPACITY: usize = 10000;

impl InMemoryTracker {
    /// Create a new [`InMemoryTracker`] with an [`EntityManager`].
    pub fn new(entity_manager: Arc<EntityManager>) -> Self {
        Self {
            entity_manager,
            state: Mutex::new(TrackedState::new()),
        }
    }

    fn add_event(&self, event: EventCommon) {
        let mut state_guard = self.state.lock().unwrap();
        state_guard.add_event(event);
    }

    fn time(&self) -> f64 {
        self.entity_manager.time()
    }

    /// Get the [`Tag`] for the specified simulation entity/object.
    pub fn tag_for_name(&self, name: &str) -> Option<Tag> {
        let state_guard = self.state.lock().unwrap();
        state_guard.tag_for_name(name)
    }

    /// Return the number of packets that exited the entity specified by `tag`.
    pub fn count_egress(&self, tag: Tag) -> usize {
        let state_guard = self.state.lock().unwrap();
        state_guard.count_egress(tag)
    }

    /// Return the number of packets that entered the entity specified by `tag`.
    pub fn ingress_count(&self, tag: Tag) -> usize {
        let state_guard = self.state.lock().unwrap();
        state_guard.count_ingress(tag)
    }

    /// Return the number of packets that exited the entity specified by `tag`.
    pub fn bus_turnaround(&self, tag: Tag) -> usize {
        let state_guard = self.state.lock().unwrap();
        state_guard.bus_turnaround(tag)
    }

    /// Return the bandwidth through the specified entity.
    ///
    /// *Note*: returns None if the bandwidth cannot be calculated.
    pub fn gbps_through(&self, tag: Tag) -> Option<f64> {
        let state_guard = self.state.lock().unwrap();
        state_guard.gbps_through(tag)
    }

    /// Return the bandwidth at the ingress of the specified entity.
    ///
    /// *Note*: returns None if the bandwidth cannot be calculated.
    pub fn gbps_ingress(&self, tag: Tag) -> Option<f64> {
        let state_guard = self.state.lock().unwrap();
        state_guard.gbps_ingress(tag)
    }

    /// Return the bandwidth at the egress of the specified entity.
    ///
    /// *Note*: returns None if the bandwidth cannot be calculated.
    pub fn egress_gbps(&self, tag: Tag) -> Option<f64> {
        let state_guard = self.state.lock().unwrap();
        state_guard.gbps_egress(tag)
    }
}

/// Implementation each [`Track`] event
impl Track for InMemoryTracker {
    fn unique_tag(&self) -> Tag {
        self.entity_manager.unique_tag()
    }

    fn is_entity_enabled(&self, tag: Tag, level: log::Level) -> bool {
        self.entity_manager.is_enabled(tag, level)
    }

    fn add_entity(&self, tag: Tag, entity_name: &str) {
        self.entity_manager.add_entity(tag, entity_name);
    }

    fn enter(&self, tag: Tag, object: Tag) {
        let time = self.time();
        let enter = Event::Enter { entered: object };
        self.add_event(EventCommon::new(tag, time, enter));
    }

    fn exit(&self, tag: Tag, object: Tag) {
        let time = self.time();
        let exit = Event::Exit { exited: object };
        self.add_event(EventCommon::new(tag, time, exit));
    }

    fn create(&self, _created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        let time = self.time();
        let create = Event::Create {
            num_bytes,
            req_type,
        };
        let mut state_guard = self.state.lock().unwrap();
        state_guard.add_event(EventCommon::new(tag, time, create));
        state_guard.add_tag_to_num_bytes(tag, num_bytes);
        state_guard.add_tag_to_req_type(tag, req_type);
        state_guard.add_name_to_tag(name, tag);
    }

    /// Track when an object with the given tag is destroyed.
    fn destroy(&self, _destroyed_by: Tag, tag: Tag) {
        let time = self.time();
        let destroy = Event::Destroy;
        self.add_event(EventCommon::new(tag, time, destroy));

        // TODO: Remove items from HashMaps to save memory?
    }

    /// Track a log message of the given level.
    fn log(&self, tag: Tag, level: log::Level, msg: std::fmt::Arguments) {
        let time = self.time();
        let log = Event::Log {
            level,
            text: format!("{msg}"),
        };
        self.add_event(EventCommon::new(tag, time, log));
    }

    fn time(&self, _set_by: Tag, time_ns: f64) {
        self.entity_manager.set_time(time_ns);
    }

    fn shutdown(&self) {
        // Do nothing
    }
}
