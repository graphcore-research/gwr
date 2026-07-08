// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use gwr_track::Id;
use gwr_track::entity::Capacity;
use gwr_track::trace_visitor::{TraceVisitor, process_capnp};

use crate::app::{CHUNK_SIZE, EventLine};
use crate::filter::Filter;
use crate::renderer::Renderer;
use crate::rocket::SHARED_STATE;

struct BinLoader {
    renderer: Arc<Mutex<Renderer>>,
    filter: Arc<Mutex<Filter>>,
    events: Option<Vec<EventLine>>,
    id_to_fullness: HashMap<u64, u64>,
    id_is_source: HashMap<u64, bool>,
    id_to_name: Option<HashMap<u64, String>>,
    id_to_details: Option<HashMap<u64, String>>,
    id_to_req_type: Option<HashMap<u64, u8>>,
    group_memberships: HashMap<u64, u64>,
    activity_lanes: HashMap<u64, u64>,
    current_time_ns: f64,
}

impl BinLoader {
    fn new(renderer: Arc<Mutex<Renderer>>, filter: Arc<Mutex<Filter>>) -> Self {
        Self {
            renderer,
            filter,
            events: Some(Vec::with_capacity(CHUNK_SIZE)),
            id_to_fullness: HashMap::new(),
            id_is_source: HashMap::new(),
            id_to_name: Some(HashMap::new()),
            id_to_details: Some(HashMap::new()),
            id_to_req_type: Some(HashMap::new()),
            group_memberships: HashMap::new(),
            activity_lanes: HashMap::new(),
            current_time_ns: 0.0,
        }
    }

    /// Add an individual event. If required, push the current events to the
    /// renderer.
    fn add_event(&mut self, event_line: EventLine) {
        self.events.as_mut().unwrap().push(event_line);

        if self.events.as_ref().unwrap().len() == CHUNK_SIZE {
            self.move_state_to_renderer();
        }
    }

    /// Done with processing the file, push remaining events to renderer
    fn finish(&mut self) {
        self.move_state_to_renderer();
    }

    /// Move all events seen so far to the renderer
    fn move_state_to_renderer(&mut self) {
        self.renderer
            .lock()
            .unwrap()
            .add_chunk(self.events.take().unwrap());
        let id_to_name = self.id_to_name.take().unwrap();
        let id_to_details = self.id_to_details.take().unwrap();
        self.renderer
            .lock()
            .unwrap()
            .extend_id_to_name(id_to_name.clone());
        self.renderer
            .lock()
            .unwrap()
            .extend_id_to_details(id_to_details.clone());
        self.filter.lock().unwrap().extend_id_to_name(id_to_name);
        self.filter
            .lock()
            .unwrap()
            .extend_id_to_details(id_to_details);

        self.events = Some(Vec::with_capacity(CHUNK_SIZE));
        self.id_to_name = Some(HashMap::new());
        self.id_to_details = Some(HashMap::new());
    }
}

/// The `TraceVisitor` trait is the interface that allows a user to see all the
/// events as a binary file is processed
impl TraceVisitor for BinLoader {
    fn log(&mut self, id: Id, level: log::Level, message: &str) {
        self.add_event(EventLine::Log {
            level,
            id: id.0,
            msg: message.to_owned(),
            time: self.current_time_ns,
        });
    }

    fn create_entity(&mut self, _created_by: Id, id: Id, name: &str) {
        SHARED_STATE
            .lock()
            .unwrap()
            .entity_names
            .push(format!("{name}={id}"));
        self.id_to_name
            .as_mut()
            .unwrap()
            .insert(id.0, name.to_owned());
        self.add_event(EventLine::Create {
            id: id.0,
            time: self.current_time_ns,
        });
    }

    fn create_monitor(&mut self, _created_by: Id, id: Id, name: &str) {
        SHARED_STATE
            .lock()
            .unwrap()
            .entity_names
            .push(format!("{name}={id}"));
        self.id_to_name
            .as_mut()
            .unwrap()
            .insert(id.0, name.to_owned());
        self.add_event(EventLine::Create {
            id: id.0,
            time: self.current_time_ns,
        });
    }

    fn create_lane(&mut self, _created_by: Id, id: Id, name: &str) {
        SHARED_STATE
            .lock()
            .unwrap()
            .entity_names
            .push(format!("{name}={id}"));
        self.id_to_name
            .as_mut()
            .unwrap()
            .insert(id.0, name.to_owned());
        self.add_event(EventLine::Create {
            id: id.0,
            time: self.current_time_ns,
        });
    }

    fn create_group(&mut self, _created_by: Id, _id: Id, _name: &str) {}

    fn create_object(
        &mut self,
        _created_by: Id,
        id: Id,
        _size: usize,
        units: &str,
        req_type: u8,
        details: &str,
    ) {
        SHARED_STATE
            .lock()
            .unwrap()
            .entity_names
            .push(format!("object={id}: {details}"));
        self.id_to_name
            .as_mut()
            .unwrap()
            .insert(id.0, "object".to_owned());
        self.id_to_details.as_mut().unwrap().insert(
            id.0,
            if units.is_empty() {
                details.to_owned()
            } else {
                format!("{details} [{units}]")
            },
        );
        self.id_to_req_type.as_mut().unwrap().insert(id.0, req_type);
        self.add_event(EventLine::Create {
            id: id.0,
            time: self.current_time_ns,
        });
    }

    fn destroy(&mut self, _destroyed_by: Id, _id: Id) {}

    fn connect(&mut self, connect_from: Id, connect_to: Id) {
        SHARED_STATE
            .lock()
            .unwrap()
            .connections
            .push(format!("{connect_from} -> {connect_to}").to_string());
        let time = self.current_time_ns;
        self.add_event(EventLine::Connect {
            from_id: connect_from.0,
            to_id: connect_to.0,
            time,
        });
    }

    fn enter(&mut self, id: Id, entered: Id) {
        // Add the fullness of 0 if not already there.
        let fullness = {
            let fullness = self.id_to_fullness.entry(id.0).or_insert(0);
            if *fullness == 0 {
                // This is a standard block
                self.id_is_source.insert(id.0, true);
            }

            if *self.id_is_source.get(&id.0).unwrap() {
                *fullness += 1;
            } else {
                *fullness = fullness.saturating_sub(1);
            }
            *fullness
        };
        let time = self.current_time_ns;
        self.add_event(EventLine::Enter {
            id: id.0,
            entered: entered.0,
            fullness,
            time,
        });
    }

    fn exit(&mut self, id: Id, exited: Id) {
        // Add the fullness of 0 if not already there (a source only ever has exit
        // events)
        let fullness = {
            let fullness = self.id_to_fullness.entry(id.0).or_insert(0);
            if *fullness == 0 {
                // This is a source so never sees Enter, only Exit
                self.id_is_source.insert(id.0, false);
            }

            if *self.id_is_source.get(&id.0).unwrap() {
                *fullness = fullness.saturating_sub(1);
            } else {
                *fullness += 1;
            }
            *fullness
        };
        let time = self.current_time_ns;
        self.add_event(EventLine::Exit {
            id: id.0,
            exited: exited.0,
            fullness,
            time,
        });
    }

    fn value(&mut self, id: Id, value: f64) {
        let time = self.current_time_ns;
        self.add_event(EventLine::Value {
            id: id.0,
            value,
            time,
        });
    }

    fn add_to_group(&mut self, activity: Id, group_id: Id) {
        self.group_memberships.insert(activity.0, group_id.0);
    }

    fn remove_from_group(&mut self, activity: Id, group_id: Id) {
        if self.group_memberships.get(&activity.0) == Some(&group_id.0) {
            self.group_memberships.remove(&activity.0);
        }
    }

    fn begin_activity(&mut self, activity: Id, lane: Id, name: &str) {
        self.activity_lanes.insert(activity.0, lane.0);
        let correlation_id = self.group_memberships.get(&activity.0).copied();
        self.add_event(EventLine::ActivityBegin {
            id: lane.0,
            name: name.to_owned(),
            correlation_id,
            time: self.current_time_ns,
        });
    }

    fn end_activity(&mut self, activity: Id) {
        if let Some(lane) = self.activity_lanes.remove(&activity.0) {
            self.add_event(EventLine::ActivityEnd {
                id: lane,
                time: self.current_time_ns,
            });
        }
    }

    fn capacity(&mut self, id: Id, capacity: Capacity) {
        SHARED_STATE
            .lock()
            .unwrap()
            .capacities
            .push(format!("{id}={},{}", capacity.value, capacity.units));
        self.renderer
            .lock()
            .unwrap()
            .set_capacity(id.0, capacity.value as u64, capacity.units);
    }

    fn time(&mut self, _id: Id, time_ns: f64) {
        self.current_time_ns = time_ns;
    }
}

pub fn start_background_load(
    bin_file_path: &Path,
    renderer: Arc<Mutex<Renderer>>,
    filter: Arc<Mutex<Filter>>,
) {
    let file = match File::open(bin_file_path) {
        Ok(file) => file,
        Err(e) => {
            println!("Error: {e}");
            return;
        }
    };

    thread::spawn(move || {
        let reader = BufReader::new(file);
        let mut bin_loader = BinLoader::new(renderer, filter);
        process_capnp(reader, &mut bin_loader);
        bin_loader.finish();
    });
}
