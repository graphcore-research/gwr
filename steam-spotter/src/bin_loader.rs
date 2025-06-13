// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, Mutex};
use std::thread;

use steam_track::Tag;
use steam_track::trace_visitor::{TraceVisitor, process_capnp};

use crate::app::{CHUNK_SIZE, EventLine};
use crate::filter::Filter;
use crate::renderer::Renderer;

struct BinLoader {
    renderer: Arc<Mutex<Renderer>>,
    filter: Arc<Mutex<Filter>>,
    events: Option<Vec<EventLine>>,
    tag_to_fullness: HashMap<u64, u64>,
    tag_is_source: HashMap<u64, bool>,
    tag_to_name: Option<HashMap<u64, String>>,
    tag_to_num_bytes: Option<HashMap<u64, usize>>,
    tag_to_req_type: Option<HashMap<u64, i8>>,
    current_time_ns: f64,
}

impl BinLoader {
    fn new(renderer: Arc<Mutex<Renderer>>, filter: Arc<Mutex<Filter>>) -> Self {
        Self {
            renderer,
            filter,
            events: Some(Vec::with_capacity(CHUNK_SIZE)),
            tag_to_fullness: HashMap::new(),
            tag_is_source: HashMap::new(),
            tag_to_name: Some(HashMap::new()),
            tag_to_num_bytes: Some(HashMap::new()),
            tag_to_req_type: Some(HashMap::new()),
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
        let tag_to_name = self.tag_to_name.take().unwrap();
        self.renderer
            .lock()
            .unwrap()
            .extend_tag_to_name(tag_to_name.clone());
        self.filter.lock().unwrap().extend_tag_to_name(tag_to_name);

        self.events = Some(Vec::with_capacity(CHUNK_SIZE));
        self.tag_to_name = Some(HashMap::new());
    }
}

/// The `TraceVisitor` trait is the interface that allows a user to see all the
/// events as a binary file is processed
impl TraceVisitor for BinLoader {
    fn log(&mut self, tag: Tag, level: log::Level, message: &str) {
        self.add_event(EventLine::Log {
            level,
            tag: tag.0,
            msg: message.to_owned(),
            time: self.current_time_ns,
        });
    }

    fn create(&mut self, _created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        self.tag_to_name
            .as_mut()
            .unwrap()
            .insert(tag.0, name.to_owned());
        self.tag_to_num_bytes
            .as_mut()
            .unwrap()
            .insert(tag.0, num_bytes);
        self.tag_to_req_type
            .as_mut()
            .unwrap()
            .insert(tag.0, req_type);
        self.add_event(EventLine::Create {
            tag: tag.0,
            time: self.current_time_ns,
        });
    }

    fn destroy(&mut self, _destroyed_by: Tag, _tag: Tag) {}

    fn enter(&mut self, tag: Tag, entered: Tag) {
        // Add the fullness of 0 if not already there.
        let fullness = {
            let fullness = self.tag_to_fullness.entry(tag.0).or_insert(0);
            if *fullness == 0 {
                // This is a standard block
                self.tag_is_source.insert(tag.0, true);
            }

            if *self.tag_is_source.get(&tag.0).unwrap() {
                *fullness += 1;
            } else {
                *fullness -= 1;
            }
            *fullness
        };
        let time = self.current_time_ns;
        self.add_event(EventLine::Enter {
            tag: tag.0,
            entered: entered.0,
            fullness,
            time,
        });
    }

    fn exit(&mut self, tag: Tag, exited: Tag) {
        // Add the fullness of 0 if not already there (a source only ever has exit
        // events)
        let fullness = {
            let fullness = self.tag_to_fullness.entry(tag.0).or_insert(0);
            if *fullness == 0 {
                // This is a source so never sees Enter, only Exit
                self.tag_is_source.insert(tag.0, false);
            }

            if *self.tag_is_source.get(&tag.0).unwrap() {
                *fullness -= 1;
            } else {
                *fullness += 1;
            }
            *fullness
        };
        let time = self.current_time_ns;
        self.add_event(EventLine::Exit {
            tag: tag.0,
            exited: exited.0,
            fullness,
            time,
        });
    }

    fn time(&mut self, _tag: Tag, time_ns: f64) {
        self.current_time_ns = time_ns;
    }
}

pub fn start_background_load(
    bin_file_path: &str,
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
