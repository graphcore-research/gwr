// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
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
    enter_re: Regex,
    exit_re: Regex,
    time_re: Regex,
    text_re: Regex,

    current_time: f64,
}

impl LogParser {
    fn new() -> Self {
        Self {
            log_line_re: Regex::new(r"(?<tag>\d+):(?<level>[^ :]+): (?<msg>.*)$").unwrap(),

            connect_re: Regex::new(r"(\d+): connect to (\d+)$").unwrap(),
            create_re: Regex::new(
                r"(?<by>\d+): created (?<tag>\d+), (?<name>.*), (?<type>.*), (?<bytes>.*) bytes$",
            )
            .unwrap(),
            enter_re: Regex::new(r"(\d+): enter (\d+)$").unwrap(),
            exit_re: Regex::new(r"(\d+): exit (\d+)$").unwrap(),
            time_re: Regex::new(r"\d+: set time to ([^n]+)ns").unwrap(),
            text_re: Regex::new(r"(\d+): (\d+) entered$").unwrap(),

            current_time: 0.0,
        }
    }

    fn parse_line(
        &mut self,
        full_line: &str,
        tag_to_name: &mut HashMap<u64, String>,
        tag_to_fullness: &mut HashMap<u64, u64>,
        tag_is_source: &mut HashMap<u64, bool>,
    ) -> EventLine {
        match self.log_line_re.captures(full_line) {
            Some(e) => {
                let tag_str = e.name("tag").unwrap().as_str();
                let tag = tag_str.parse().unwrap();
                let level_str = e.name("level").unwrap().as_str();
                let msg = e.name("msg").unwrap().as_str();
                EventLine::Log {
                    level: Level::from_str(level_str).unwrap(),
                    tag,
                    msg: msg.to_owned(),
                    time: self.current_time,
                }
            }
            None => self.parse_msg(full_line, tag_to_name, tag_to_fullness, tag_is_source),
        }
    }

    fn parse_msg(
        &mut self,
        msg: &str,
        tag_to_name: &mut HashMap<u64, String>,
        tag_to_fullness: &mut HashMap<u64, u64>,
        tag_is_source: &mut HashMap<u64, bool>,
    ) -> EventLine {
        if let Some(e) = self.enter_re.captures(msg) {
            let tag_str = e.get(1).unwrap().as_str();
            let tag = tag_str.parse().unwrap();
            let entered_tag_str = e.get(2).unwrap().as_str();

            // Add the fullness of 0 if not already there.
            let fullness = tag_to_fullness.entry(tag).or_insert(0);
            if *fullness == 0 {
                // This is a standard block
                tag_is_source.insert(tag, true);
            }

            if *tag_is_source.get(&tag).unwrap() {
                *fullness += 1;
            } else {
                *fullness -= 1;
            }

            return EventLine::Enter {
                tag,
                fullness: *fullness,
                entered: entered_tag_str.parse().unwrap(),
                time: self.current_time,
            };
        } else if let Some(e) = self.exit_re.captures(msg) {
            let tag_str = e.get(1).unwrap().as_str();
            let tag = tag_str.parse().unwrap();
            let exited_tag_str = e.get(2).unwrap().as_str();

            // Add the fullness of 0 if not already there (a source only ever has exit
            // events)
            let fullness = tag_to_fullness.entry(tag).or_insert(0);
            if *fullness == 0 {
                // This is a source so never sees Enter, only Exit
                tag_is_source.insert(tag, false);
            }

            if *tag_is_source.get(&tag).unwrap() {
                *fullness -= 1;
            } else {
                *fullness += 1;
            }

            return EventLine::Exit {
                tag,
                fullness: *fullness,
                exited: exited_tag_str.parse().unwrap(),
                time: self.current_time,
            };
        } else if let Some(e) = self.time_re.captures(msg) {
            let time_str = e.get(1).unwrap().as_str();
            self.current_time = time_str.parse().unwrap();
        } else if let Some(e) = self.text_re.captures(msg) {
            let level_str = e.get(1).unwrap().as_str();
            let tag_str = e.get(2).unwrap().as_str();
            let text_str = e.get(3).unwrap().as_str();
            return EventLine::Log {
                level: log::Level::from_str(level_str).unwrap(),
                tag: tag_str.parse().unwrap(),
                msg: text_str.to_owned(),
                time: self.current_time,
            };
        } else if let Some(e) = self.create_re.captures(msg) {
            let tag_str = e.name("tag").unwrap().as_str();
            let tag = tag_str.parse().unwrap();
            let name_str = e.name("name").unwrap().as_str();
            let name = name_str.to_owned();

            SHARED_STATE
                .lock()
                .unwrap()
                .entity_names
                .push(format!("{name}={tag_str}"));

            tag_to_name.insert(tag, name);

            return EventLine::Create {
                tag,
                time: self.current_time,
            };
        } else if let Some(e) = self.connect_re.captures(msg) {
            let from_tag_str = e.get(1).unwrap().as_str();
            let from_tag = from_tag_str.parse().unwrap();
            let to_tag_str = e.get(2).unwrap().as_str();
            let to_tag = to_tag_str.parse().unwrap();

            SHARED_STATE
                .lock()
                .unwrap()
                .connections
                .push(format!("{from_tag_str} -> {to_tag_str}").to_string());

            return EventLine::Connect {
                from_tag,
                to_tag,
                time: self.current_time,
            };
        }

        EventLine::Log {
            level: log::Level::Trace,
            tag: 0,
            msg: msg.to_owned(),
            time: self.current_time,
        }
    }
}

pub fn start_background_load(
    log_file_path: &str,
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
        let mut tag_to_fullness = HashMap::new();

        // The BarChart widget plots u64 values. As a result, we can't simply have Exit
        // mean the fullness is decremented as otherwise Sources will always
        // have negative fullnesses. As a result, we detect the first operation
        // on an entity and decide if it is a source.
        let mut tag_is_source = HashMap::new();

        let reader = BufReader::new(file);
        for chunk in &reader.lines().chunks(CHUNK_SIZE) {
            let mut events = Vec::with_capacity(CHUNK_SIZE);
            let mut tag_to_name = HashMap::new();

            for l in chunk {
                match l {
                    Ok(line) => events.push(parser.parse_line(
                        line.as_str(),
                        &mut tag_to_name,
                        &mut tag_to_fullness,
                        &mut tag_is_source,
                    )),
                    Err(e) => {
                        let err_line = EventLine::Log {
                            level: log::Level::Error,
                            tag: 0,
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
                .extend_tag_to_name(tag_to_name.clone());
            filter.lock().unwrap().extend_tag_to_name(tag_to_name);
        }
    });
}
