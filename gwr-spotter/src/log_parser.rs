// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use itertools::Itertools;
use log::Level;
use regex::Regex;

use crate::app::{CHUNK_SIZE, EventLine};
use crate::filter::Filter;
use crate::renderer::Renderer;
use crate::rocket::SHARED_STATE;

struct LogParser {
    log_line_re: Regex,
    connect_re: Regex,
    create_entity_re: Regex,
    create_monitor_re: Regex,
    create_object_re: Regex,
    enter_re: Regex,
    exit_re: Regex,
    value_re: Regex,
    time_re: Regex,
    text_re: Regex,

    current_time: Duration,
}

impl LogParser {
    fn new() -> Self {
        Self {
            log_line_re: Regex::new(r"(?<id>\d+):(?<level>[^ :]+): (?<msg>.*)$").unwrap(),

            connect_re: Regex::new(r"(\d+): connect to (\d+)$").unwrap(),
            create_entity_re: Regex::new(
                r"(?<by>\d+): created entity (?<id>\d+), (?<name>.*)$",
            )
            .unwrap(),
            create_monitor_re: Regex::new(
                r"(?<by>\d+): created monitor (?<id>\d+), (?<name>.*)$",
            )
            .unwrap(),
            create_object_re: Regex::new(
                r"(?<by>\d+): created object (?<id>\d+), (?<type>\d+), (?<size>\d+), (?<units>.*), (?<details>.*)$",
            )
            .unwrap(),
            enter_re: Regex::new(r"(\d+): enter (\d+)$").unwrap(),
            exit_re: Regex::new(r"(\d+): exit (\d+)$").unwrap(),
            value_re: Regex::new(r"(\d+): value (.*)$").unwrap(),
            time_re: Regex::new(r"\d+: set time to ([^n]+)ns").unwrap(),
            text_re: Regex::new(r"(\d+): (\d+) entered$").unwrap(),

            current_time: Duration::new(0, 0),
        }
    }

    fn parse_line(
        &mut self,
        full_line: &str,
        id_to_name: &mut HashMap<u64, String>,
        id_to_details: &mut HashMap<u64, String>,
        id_to_fullness: &mut HashMap<u64, u64>,
        id_is_source: &mut HashMap<u64, bool>,
    ) -> EventLine {
        match self.log_line_re.captures(full_line) {
            Some(e) => {
                let id_str = e.name("id").unwrap().as_str();
                let id = id_str.parse().unwrap();
                let level_str = e.name("level").unwrap().as_str();
                let msg = e.name("msg").unwrap().as_str();
                EventLine::Log {
                    level: Level::from_str(level_str).unwrap(),
                    id,
                    msg: msg.to_owned(),
                    time: self.current_time,
                }
            }
            None => self.parse_msg(
                full_line,
                id_to_name,
                id_to_details,
                id_to_fullness,
                id_is_source,
            ),
        }
    }

    fn parse_msg(
        &mut self,
        msg: &str,
        id_to_name: &mut HashMap<u64, String>,
        id_to_details: &mut HashMap<u64, String>,
        id_to_fullness: &mut HashMap<u64, u64>,
        id_is_source: &mut HashMap<u64, bool>,
    ) -> EventLine {
        if let Some(event) = self.parse_enter(msg, id_to_fullness, id_is_source) {
            return event;
        }
        if let Some(event) = self.parse_exit(msg, id_to_fullness, id_is_source) {
            return event;
        }
        if let Some(event) = self.parse_value(msg) {
            return event;
        }
        if self.parse_time(msg) {
            return EventLine::Log {
                level: log::Level::Trace,
                id: 0,
                msg: msg.to_owned(),
                time: self.current_time,
            };
        }
        if let Some(event) = self.parse_text_log(msg) {
            return event;
        }
        if let Some(event) = self.parse_create_entity(msg, id_to_name) {
            return event;
        }
        if let Some(event) = self.parse_create_monitor(msg, id_to_name) {
            return event;
        }
        if let Some(event) = self.parse_create_object(msg, id_to_name, id_to_details) {
            return event;
        }
        if let Some(event) = self.parse_connect(msg) {
            return event;
        }

        EventLine::Log {
            level: log::Level::Trace,
            id: 0,
            msg: msg.to_owned(),
            time: self.current_time,
        }
    }

    fn parse_enter(
        &self,
        msg: &str,
        id_to_fullness: &mut HashMap<u64, u64>,
        id_is_source: &mut HashMap<u64, bool>,
    ) -> Option<EventLine> {
        let e = self.enter_re.captures(msg)?;
        let id: u64 = e.get(1).unwrap().as_str().parse().unwrap();
        let entered: u64 = e.get(2).unwrap().as_str().parse().unwrap();

        // Add the fullness of 0 if not already there.
        let fullness = id_to_fullness.entry(id).or_insert(0);
        if *fullness == 0 {
            // This is a standard block
            id_is_source.insert(id, true);
        }

        if *id_is_source.get(&id).unwrap() {
            *fullness += 1;
        } else {
            *fullness -= 1;
        }

        Some(EventLine::Enter {
            id,
            fullness: *fullness,
            entered,
            time: self.current_time,
        })
    }

    fn parse_exit(
        &self,
        msg: &str,
        id_to_fullness: &mut HashMap<u64, u64>,
        id_is_source: &mut HashMap<u64, bool>,
    ) -> Option<EventLine> {
        let e = self.exit_re.captures(msg)?;
        let id: u64 = e.get(1).unwrap().as_str().parse().unwrap();
        let exited: u64 = e.get(2).unwrap().as_str().parse().unwrap();

        // Add the fullness of 0 if not already there (a source only ever has exit
        // events)
        let fullness = id_to_fullness.entry(id).or_insert(0);
        if *fullness == 0 {
            // This is a source so never sees Enter, only Exit
            id_is_source.insert(id, false);
        }

        if *id_is_source.get(&id).unwrap() {
            *fullness -= 1;
        } else {
            *fullness += 1;
        }

        Some(EventLine::Exit {
            id,
            fullness: *fullness,
            exited,
            time: self.current_time,
        })
    }

    fn parse_value(&self, msg: &str) -> Option<EventLine> {
        let e = self.value_re.captures(msg)?;
        let id: u64 = e.get(1).unwrap().as_str().parse().unwrap();
        let value = e.get(2).unwrap().as_str().parse().unwrap();
        Some(EventLine::Value {
            id,
            value,
            time: self.current_time,
        })
    }

    fn parse_time(&mut self, msg: &str) -> bool {
        let Some(e) = self.time_re.captures(msg) else {
            return false;
        };
        let time_str = e.get(1).unwrap().as_str();
        let nanos: u128 = time_str.parse().unwrap();
        self.current_time = Duration::from_nanos_u128(nanos);
        true
    }

    fn parse_text_log(&self, msg: &str) -> Option<EventLine> {
        let e = self.text_re.captures(msg)?;
        let level_str = e.get(1).unwrap().as_str();
        let id_str = e.get(2).unwrap().as_str();
        let text_str = e.get(3).unwrap().as_str();
        Some(EventLine::Log {
            level: log::Level::from_str(level_str).unwrap(),
            id: id_str.parse().unwrap(),
            msg: text_str.to_owned(),
            time: self.current_time,
        })
    }

    fn parse_create_entity(
        &self,
        msg: &str,
        id_to_name: &mut HashMap<u64, String>,
    ) -> Option<EventLine> {
        let e = self.create_entity_re.captures(msg)?;
        let id_str = e.name("id").unwrap().as_str();
        let id = id_str.parse().unwrap();
        let name = e.name("name").unwrap().as_str().to_owned();

        SHARED_STATE
            .lock()
            .unwrap()
            .entity_names
            .push(format!("{name}={id_str}"));

        id_to_name.insert(id, name);

        Some(EventLine::Create {
            id,
            time: self.current_time,
        })
    }

    fn parse_create_monitor(
        &self,
        msg: &str,
        id_to_name: &mut HashMap<u64, String>,
    ) -> Option<EventLine> {
        let e = self.create_monitor_re.captures(msg)?;
        let id_str = e.name("id").unwrap().as_str();
        let id = id_str.parse().unwrap();
        let name = e.name("name").unwrap().as_str().to_owned();

        SHARED_STATE
            .lock()
            .unwrap()
            .entity_names
            .push(format!("{name}={id_str}"));

        id_to_name.insert(id, name);

        Some(EventLine::Create {
            id,
            time: self.current_time,
        })
    }

    fn parse_create_object(
        &self,
        msg: &str,
        id_to_name: &mut HashMap<u64, String>,
        id_to_details: &mut HashMap<u64, String>,
    ) -> Option<EventLine> {
        let e = self.create_object_re.captures(msg)?;
        let id_str = e.name("id").unwrap().as_str();
        let id = id_str.parse().unwrap();
        let units = e.name("units").unwrap().as_str().to_owned();
        let details = e.name("details").unwrap().as_str().to_owned();

        SHARED_STATE
            .lock()
            .unwrap()
            .entity_names
            .push(format!("object={id_str}: {details}"));

        id_to_name.insert(id, "object".to_owned());
        id_to_details.insert(
            id,
            if units.is_empty() {
                details
            } else {
                format!("{details} [{units}]")
            },
        );

        Some(EventLine::Create {
            id,
            time: self.current_time,
        })
    }

    fn parse_connect(&self, msg: &str) -> Option<EventLine> {
        let e = self.connect_re.captures(msg)?;
        let from_id_str = e.get(1).unwrap().as_str();
        let from_id = from_id_str.parse().unwrap();
        let to_id_str = e.get(2).unwrap().as_str();
        let to_id = to_id_str.parse().unwrap();

        SHARED_STATE
            .lock()
            .unwrap()
            .connections
            .push(format!("{from_id_str} -> {to_id_str}").to_string());

        Some(EventLine::Connect {
            from_id,
            to_id,
            time: self.current_time,
        })
    }
}

pub fn start_background_load(
    log_file_path: &Path,
    renderer: Arc<Mutex<Renderer>>,
    filter: Arc<Mutex<Filter>>,
) {
    let file = match File::open(log_file_path) {
        Ok(file) => file,
        Err(e) => {
            println!("Error: {e}");
            return;
        }
    };

    thread::spawn(move || {
        let mut parser = LogParser::new();

        // Keep track of the fullness of each entity so that Enter/Exit events can
        // contain the absolute value
        let mut id_to_fullness = HashMap::new();

        // The BarChart widget plots u64 values. As a result, we can't simply have Exit
        // mean the fullness is decremented as otherwise Sources will always
        // have negative fullnesses. As a result, we detect the first operation
        // on an entity and decide if it is a source.
        let mut id_is_source = HashMap::new();

        let reader = BufReader::new(file);
        for chunk in &reader.lines().chunks(CHUNK_SIZE) {
            let mut events = Vec::with_capacity(CHUNK_SIZE);
            let mut id_to_name = HashMap::new();
            let mut id_to_details = HashMap::new();

            for l in chunk {
                match l {
                    Ok(line) => events.push(parser.parse_line(
                        line.as_str(),
                        &mut id_to_name,
                        &mut id_to_details,
                        &mut id_to_fullness,
                        &mut id_is_source,
                    )),
                    Err(e) => {
                        let err_line = EventLine::Log {
                            level: log::Level::Error,
                            id: 0,
                            msg: e.to_string(),
                            time: Duration::new(0, 0),
                        };
                        events.push(err_line);
                    }
                }
            }

            renderer.lock().unwrap().add_chunk(events);
            renderer
                .lock()
                .unwrap()
                .extend_id_to_name(id_to_name.clone());
            renderer
                .lock()
                .unwrap()
                .extend_id_to_details(id_to_details.clone());
            filter.lock().unwrap().extend_id_to_name(id_to_name);
            filter.lock().unwrap().extend_id_to_details(id_to_details);
        }
    });
}
