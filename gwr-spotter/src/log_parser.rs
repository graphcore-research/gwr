// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;

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
    create_re: Regex,
    begin_activity_re: Regex,
    end_activity_re: Regex,
    add_to_group_re: Regex,
    remove_from_group_re: Regex,
    enter_re: Regex,
    exit_re: Regex,
    value_re: Regex,
    time_re: Regex,
    text_re: Regex,
    group_memberships: HashMap<u64, u64>,
    activity_lanes: HashMap<u64, u64>,

    current_time: f64,
}

impl LogParser {
    fn new() -> Self {
        Self {
            log_line_re: Regex::new(r"(?<id>\d+):(?<level>[^ :]+): (?<msg>.*)$").unwrap(),

            connect_re: Regex::new(r"(\d+): connect to (\d+)$").unwrap(),
            create_re: Regex::new(r"(?<by>\d+): created (?<kind>\w+) (?<rest>.*)$").unwrap(),
            add_to_group_re: Regex::new(r"(?<id>\d+): added to group (?<group_id>\d+)$").unwrap(),
            remove_from_group_re: Regex::new(r"(?<id>\d+): removed from group (?<group_id>\d+)$")
                .unwrap(),
            begin_activity_re: Regex::new(
                r"(?<id>\d+): activity begin (?<name>.*) on lane (?<lane>\d+)$",
            )
            .unwrap(),
            end_activity_re: Regex::new(r"(?<id>\d+): activity end$").unwrap(),
            enter_re: Regex::new(r"(\d+): enter (\d+)$").unwrap(),
            exit_re: Regex::new(r"(\d+): exit (\d+)$").unwrap(),
            value_re: Regex::new(r"(\d+): value (.*)$").unwrap(),
            time_re: Regex::new(r"\d+: set time to ([^n]+)ns").unwrap(),
            text_re: Regex::new(r"(\d+): (\d+) entered$").unwrap(),
            group_memberships: HashMap::new(),
            activity_lanes: HashMap::new(),

            current_time: 0.0,
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
        if let Some(event) = self.parse_create(msg, id_to_name, id_to_details) {
            return event;
        }
        if let Some(event) = self.parse_add_to_group(msg) {
            return event;
        }
        if let Some(event) = self.parse_remove_from_group(msg) {
            return event;
        }
        if let Some(event) = self.parse_begin_activity(msg) {
            return event;
        }
        if let Some(event) = self.parse_end_activity(msg) {
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
        self.current_time = time_str.parse().unwrap();
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

    fn parse_create(
        &self,
        msg: &str,
        id_to_name: &mut HashMap<u64, String>,
        id_to_details: &mut HashMap<u64, String>,
    ) -> Option<EventLine> {
        let e = self.create_re.captures(msg)?;
        let kind = e.name("kind").unwrap().as_str();
        let rest = e.name("rest").unwrap().as_str();

        match kind {
            "entity" | "monitor" | "lane" | "group" | "activity" => {
                self.parse_named_create(rest, id_to_name)
            }
            "object" => self.parse_object_create(rest, id_to_name, id_to_details),
            _ => None,
        }
    }

    fn parse_named_create(
        &self,
        rest: &str,
        id_to_name: &mut HashMap<u64, String>,
    ) -> Option<EventLine> {
        let (id_str, name) = rest.split_once(", ")?;
        let id = id_str.parse().unwrap();
        let name = name.to_owned();

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

    fn parse_object_create(
        &self,
        rest: &str,
        id_to_name: &mut HashMap<u64, String>,
        id_to_details: &mut HashMap<u64, String>,
    ) -> Option<EventLine> {
        let mut fields = rest.splitn(5, ", ");
        let id_str = fields.next()?;
        let _req_type = fields.next()?;
        let _size = fields.next()?;
        let units = fields.next()?;
        let details = fields.next()?.to_owned();
        let id = id_str.parse().unwrap();

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

    fn parse_add_to_group(&mut self, msg: &str) -> Option<EventLine> {
        let e = self.add_to_group_re.captures(msg)?;
        let id = e.name("id").unwrap().as_str().parse().unwrap();
        let group_id = e.name("group_id").unwrap().as_str().parse().unwrap();
        self.group_memberships.insert(id, group_id);
        Some(EventLine::Log {
            level: log::Level::Trace,
            id,
            msg: msg.to_owned(),
            time: self.current_time,
        })
    }

    fn parse_remove_from_group(&mut self, msg: &str) -> Option<EventLine> {
        let e = self.remove_from_group_re.captures(msg)?;
        let id = e.name("id").unwrap().as_str().parse().unwrap();
        let group_id = e.name("group_id").unwrap().as_str().parse().unwrap();
        if self.group_memberships.get(&id) == Some(&group_id) {
            self.group_memberships.remove(&id);
        }
        Some(EventLine::Log {
            level: log::Level::Trace,
            id,
            msg: msg.to_owned(),
            time: self.current_time,
        })
    }

    fn parse_begin_activity(&mut self, msg: &str) -> Option<EventLine> {
        let e = self.begin_activity_re.captures(msg)?;
        let id = e.name("id").unwrap().as_str().parse().unwrap();
        let lane = e.name("lane").unwrap().as_str().parse().unwrap();
        let name = e.name("name").unwrap().as_str().to_owned();
        self.activity_lanes.insert(id, lane);
        let correlation_id = self.group_memberships.get(&id).copied();
        Some(EventLine::ActivityBegin {
            id: lane,
            name,
            correlation_id,
            time: self.current_time,
        })
    }

    fn parse_end_activity(&mut self, msg: &str) -> Option<EventLine> {
        let e = self.end_activity_re.captures(msg)?;
        let id = e.name("id").unwrap().as_str().parse().unwrap();
        let id = self.activity_lanes.remove(&id).unwrap_or(id);
        Some(EventLine::ActivityEnd {
            id,
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
                            time: 0.0,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_created_events_from_shared_prefix() {
        let parser = LogParser::new();
        let mut id_to_name = HashMap::new();
        let mut id_to_details = HashMap::new();

        let event = parser
            .parse_create(
                "12: created lane 34, top::pe0::lane::compute::0",
                &mut id_to_name,
                &mut id_to_details,
            )
            .unwrap();
        assert!(matches!(event, EventLine::Create { id: 34, .. }));
        assert_eq!(
            id_to_name.get(&34).map(String::as_str),
            Some("top::pe0::lane::compute::0")
        );

        let event = parser
            .parse_create(
                "12: created object 35, 255, 128, bytes, tensor chunk",
                &mut id_to_name,
                &mut id_to_details,
            )
            .unwrap();
        assert!(matches!(event, EventLine::Create { id: 35, .. }));
        assert_eq!(id_to_name.get(&35).map(String::as_str), Some("object"));
        assert_eq!(
            id_to_details.get(&35).map(String::as_str),
            Some("tensor chunk [bytes]")
        );
    }
}
