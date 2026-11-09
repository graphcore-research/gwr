// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Show the entity names available to timetable trace filters.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;

use clap::{ArgGroup, Parser};
use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::entity::Capacity;
use gwr_track::tracker::aka::AlternativeNames;
use gwr_track::{Id, ROOT, Track};
use regex::Regex;

fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = Cli::parse();
    let tracker = Rc::new(EntityNameTracker::new(args.filter())?);
    let track: Rc<dyn Track> = tracker.clone();
    let mut engine = Engine::new(&track);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_file(&engine, &clock, &args.platform)?);
    let graph = TimetableFile::from_file(&args.timetable)?.into_graph()?;
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform)?);
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run()?;
    timetable.check_tasks_complete()?;
    tracker.print_matches(args.show_all)
}

#[derive(Parser)]
#[command(about = "List timetable trace entities and preview a Perfetto filter")]
#[command(group(ArgGroup::new("filter").args([
    "filter_regex",
    "regex",
    "perfetto_filter_regex",
    "binary_filter_regex",
    "stdout_filter_regex",
]).multiple(false)))]
struct Cli {
    /// Platform YAML file.
    #[arg(long, default_value = "platform.yaml")]
    platform: PathBuf,

    /// Timetable YAML file.
    #[arg(long, default_value = "timetable.yaml")]
    timetable: PathBuf,

    /// Regex to preview against all registered entity names.
    #[arg(long)]
    filter_regex: Option<String>,

    /// Short spelling of --filter-regex.
    #[arg(long)]
    regex: Option<String>,

    /// Copy a Perfetto trace filter from a timetable command.
    #[arg(long)]
    perfetto_filter_regex: Option<String>,

    /// Copy a binary trace filter from a timetable command.
    #[arg(long)]
    binary_filter_regex: Option<String>,

    /// Copy a stdout filter from a timetable command.
    #[arg(long)]
    stdout_filter_regex: Option<String>,

    /// Show unmatched names as well as matches.
    #[arg(long)]
    show_all: bool,
}

impl Cli {
    fn filter(&self) -> Option<(&'static str, &str)> {
        self.filter_regex
            .as_deref()
            .map(|pattern| ("--filter-regex", pattern))
            .or_else(|| self.regex.as_deref().map(|pattern| ("--regex", pattern)))
            .or_else(|| {
                self.perfetto_filter_regex
                    .as_deref()
                    .map(|pattern| ("--perfetto-filter-regex", pattern))
            })
            .or_else(|| {
                self.binary_filter_regex
                    .as_deref()
                    .map(|pattern| ("--binary-filter-regex", pattern))
            })
            .or_else(|| {
                self.stdout_filter_regex
                    .as_deref()
                    .map(|pattern| ("--stdout-filter-regex", pattern))
            })
    }
}

struct EntityNameTracker {
    next_id: Cell<u64>,
    names: RefCell<BTreeMap<String, Option<String>>>,
    filter: Option<Regex>,
}

impl EntityNameTracker {
    fn new(filter: Option<(&str, &str)>) -> Result<Self, Box<dyn Error>> {
        let filter = filter
            .map(|(option, pattern)| {
                Regex::new(pattern).map_err(|error| {
                    io::Error::other(format!("invalid {option} {pattern:?}: {error}"))
                })
            })
            .transpose()?;
        Ok(Self {
            next_id: Cell::new(ROOT.0 + 1),
            names: RefCell::new(BTreeMap::new()),
            filter,
        })
    }

    fn print_matches(&self, show_all: bool) -> Result<(), Box<dyn Error>> {
        let names = self.names.borrow();
        let mut matched = 0;
        if let Some(filter) = &self.filter {
            println!("Filter regex: {:?}", filter.as_str());
        }
        for (name, matched_name) in names.iter() {
            if self.filter.is_some() {
                if let Some(matched_name) = matched_name {
                    matched += 1;
                    if matched_name == name {
                        println!("MATCH    {name}");
                    } else {
                        println!("MATCH    {name} (via alias {matched_name})");
                    }
                } else if show_all {
                    println!("NO MATCH {name}");
                }
            } else {
                println!("{name}");
            }
        }
        if self.filter.is_some() {
            println!("Matched {matched} of {} entities.", names.len());
            if matched == 0 {
                return Err(io::Error::other(
                    "the filter matched no entities; rerun with --show-all to inspect every name",
                )
                .into());
            }
        } else {
            println!("Listed {} entities.", names.len());
        }
        Ok(())
    }
}

impl Track for EntityNameTracker {
    fn unique_id(&self) -> Id {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        Id(id)
    }

    fn enabled_level(&self, _id: Id) -> log::Level {
        log::Level::Error
    }

    fn monitoring_window_size_for(&self, _id: Id) -> Option<u64> {
        None
    }

    fn add_entity(&self, _id: Id, name: &str, alternative_names: AlternativeNames) -> log::Level {
        let matched_name = self.filter.as_ref().and_then(|filter| {
            if filter.is_match(name) {
                Some(name.to_owned())
            } else {
                alternative_names?
                    .iter()
                    .find(|alias| filter.is_match(alias))
                    .cloned()
            }
        });
        self.names
            .borrow_mut()
            .insert(name.to_owned(), matched_name);
        log::Level::Error
    }

    fn enter(&self, _id: Id, _obj: Id) {}

    fn exit(&self, _id: Id, _obj: Id) {}

    fn value(&self, _id: Id, _value: f64) {}

    fn begin_activity(&self, _activity: Id, _lane: Id, _name: &str) {}

    fn end_activity(&self, _activity: Id) {}

    fn add_to_group(&self, _activity: Id, _group_id: Id) {}

    fn remove_from_group(&self, _activity: Id, _group_id: Id) {}

    fn capacity(&self, _id: Id, _capacity: Capacity) {}

    fn create_entity(&self, _created_by: Id, _id: Id, _name: &str) {}

    fn create_monitor(&self, _created_by: Id, _id: Id, _name: &str) {}

    fn create_lane(&self, _created_by: Id, _id: Id, _name: &str) {}

    fn create_group(&self, _created_by: Id, _id: Id, _name: &str) {}

    fn create_object(
        &self,
        _created_by: Id,
        _id: Id,
        _size: usize,
        _units: &str,
        _req_type: u8,
        _details: &str,
    ) {
    }

    fn destroy(&self, _id: Id, _obj: Id) {}

    fn connect(&self, _connect_from: Id, _connect_to: Id) {}

    fn log(&self, _id: Id, _level: log::Level, _msg: std::fmt::Arguments) {}

    fn time(&self, _set_by: Id, _time_ns: f64) {}

    fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use gwr_track::{Id, Track};

    use super::EntityNameTracker;

    #[test]
    fn records_the_alias_responsible_for_a_match() {
        let tracker = EntityNameTracker::new(Some(("--regex", "::lane::"))).unwrap();
        let aliases = vec!["top::pe0::lane::compute::0".to_owned()];
        tracker.add_entity(Id(2), "top::other", Some(&aliases));
        assert_eq!(
            tracker.names.borrow()["top::other"].as_deref(),
            Some("top::pe0::lane::compute::0")
        );
    }
}
