// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! Read and check a Cap'n Proto trace file created using tramway_track
//!
//! Checks:
//!  - all IDs used were created first
//!  - no destroyed IDs are used again

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::str::FromStr;

use log::{LevelFilter, debug, error, info, trace, warn};
use simplelog::{ConfigBuilder, SimpleLogger};
use tramway_config::multi_source_config;
use tramway_track::Id;
use tramway_track::trace_visitor::{TraceVisitor, process_capnp};

/// Decide whether message is trace!() or info!()
///
/// The user can decide what messages are trace!() or info!() depending on what
/// IDs are being displayed
macro_rules! info_trace {
    ($cond:expr, $($arg:tt)+) => (
        if $cond {
            info!($($arg)+);
        } else {
            trace!($($arg)+);
        }
    );
}

/// Decide whether message is debug!() or info!()
///
/// The user can decide what messages are debug!() or info!() depending on what
/// IDs are being displayed
macro_rules! info_debug {
    ($cond:expr, $($arg:tt)+) => (
        if $cond {
            info!($($arg)+);
        } else {
            debug!($($arg)+);
        }
    );
}

/// A tree structure to track which IDs are created where
///
/// Simply store IDs to connect to other Nodes which will be stored in a
/// separate HashMap
#[derive(Debug)]
struct Node {
    parent: Id,
    id: Id,
    children: HashSet<Id>,
}

impl Node {
    fn new(parent: Id, id: Id) -> Self {
        Self {
            parent,
            id,
            children: HashSet::new(),
        }
    }

    /// Add a child
    fn add_child(&mut self, child: Id, log_as_info: bool) {
        self.children.insert(child);
        info_debug!(log_as_info, "{} added child {}", self.id, child);
    }

    /// Remove a child and return whether the children are now empty
    fn remove_child(&mut self, child: Id, log_as_info: bool) -> bool {
        if self.children.remove(&child) {
            info_debug!(log_as_info, "{} removed child {}", self.id, child);
        } else {
            error!(
                "{} attempting to remove child {} not in children",
                self.id, child
            );
        }

        self.children.is_empty()
    }
}

/// The Checker is responsible for keeping track of all state and checking its
/// correctness
struct Checker {
    active_ids: HashMap<Id, Node>,
    destroyed: HashMap<Id, Node>,
    info_ids: HashSet<Id>,
}

impl Checker {
    fn new(info_ids: &[u64]) -> Self {
        let mut checker = Self {
            active_ids: HashMap::new(),
            destroyed: HashMap::new(),
            info_ids: HashSet::new(),
        };

        for id in info_ids {
            checker.info_ids.insert(Id(*id));
        }

        let root = Node::new(tramway_track::NO_ID, tramway_track::ROOT);
        let no_id = Node::new(tramway_track::NO_ID, tramway_track::NO_ID);

        // It is always ok to use these special identifiers from tramway_track
        checker.active_ids.insert(tramway_track::ROOT, root);
        checker.active_ids.insert(tramway_track::NO_ID, no_id);

        checker
    }

    fn check(&self, id: Id) {
        if !self.active_ids.contains_key(&id) {
            error!("Location ID {id} used when not active");
        }
    }

    fn create_id(&mut self, id: Id, created_by: Id) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.active_ids.entry(id) {
            e.insert(Node::new(created_by, id));
            self.add_child(created_by, id);
        } else {
            error!("Attempting to create existing ID {id}");
        }
    }

    fn destroy_id(&mut self, id: Id) {
        if id == tramway_track::ROOT || id == tramway_track::NO_ID {
            error!("Unable to destroy {id}");
            return;
        }

        if let Some(node) = self.active_ids.remove(&id) {
            self.remove_child(node.parent, id);

            if !node.children.is_empty() {
                info!("attempting to destroy obj {id} with active children");
                self.destroyed.insert(id, node);
            } else {
                info_debug!(self.info_ids.contains(&id), "{} destroyed", id);
            }
        } else {
            error!("attempting to destroy unknown ID {id}");
        }
    }

    fn add_child(&mut self, parent: Id, child: Id) {
        let log_as_info = self.log_as_info(child);
        if let Some(node) = self.active_ids.get_mut(&parent) {
            node.add_child(child, log_as_info);
        } else {
            error!("attempting to add {child} to unknown parent {parent}");
        }
    }

    fn remove_child(&mut self, parent: Id, child: Id) {
        let log_as_info = self.log_as_info(child);
        if let Some(parent_node) = self.active_ids.get_mut(&parent) {
            parent_node.remove_child(child, log_as_info);
        } else if let Some(parent_node) = self.destroyed.get_mut(&parent) {
            let is_empty = parent_node.remove_child(child, log_as_info);
            if is_empty {
                info_debug!(log_as_info, "{} destroyed", parent);
                self.destroyed.remove(&parent).unwrap();
            }
        } else {
            error!("attempting to remove {child} from invalid parent {parent}");
        }
    }

    fn log_as_info(&self, id: Id) -> bool {
        self.info_ids.contains(&id)
    }

    fn end_checks(&self) {
        let still_active = self
            .active_ids
            .iter()
            .filter(|&(k, _v)| *k != tramway_track::NO_ID && *k != tramway_track::ROOT);

        for (k, v) in still_active {
            warn!("ID {k} still active");
            if !v.children.is_empty() {
                warn!("  with children {:?}", v.children);
            }
        }
    }
}

impl TraceVisitor for Checker {
    fn log(&mut self, id: Id, level: log::Level, message: &str) {
        info_trace!(
            self.log_as_info(id),
            "{}:{}: Message: {}",
            id,
            level,
            message
        );

        self.check(id);
    }

    fn create(&mut self, created_by: Id, id: Id, num_bytes: usize, req_type: i8, name: &str) {
        info_trace!(
            self.log_as_info(id),
            "{}: created {}, {}, {}, {} bytes",
            created_by,
            id,
            name,
            req_type,
            num_bytes,
        );

        self.create_id(id, created_by);
        self.check(id);
    }

    fn destroy(&mut self, id: Id, destroyed_by: Id) {
        info_trace!(self.log_as_info(id), "{}: destroyed {}", destroyed_by, id,);
        self.check(id);
        self.destroy_id(id);
    }

    fn enter(&mut self, id: Id, entered: Id) {
        info_trace!(self.log_as_info(id), "{}: {} enter", id, entered);

        self.check(id);
    }

    fn exit(&mut self, id: Id, exited: Id) {
        info_trace!(self.log_as_info(id), "{}: {} exit", id, exited);

        self.check(id);
    }
}

// Structure defining the command-line arguments
#[multi_source_config]
#[command(about = "Cap'n Proto log checker.")]
struct Settings {
    /// Logging level
    #[arg(long)]
    log: Option<String>,

    /// Cap'n Proto log file to read and check
    #[arg(short = 'b', long = "bin-file")]
    bin_log_file: Option<String>,

    /// IDs to log at INFO level
    #[arg(long = "ids")]
    ids: Option<Vec<u64>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            log: Some("info".to_string()),
            bin_log_file: Some(Default::default()),
            ids: Some(Default::default()),
        }
    }
}

/// Take the command-line string and convert it to a Level
fn choose_level(lvl: &str) -> LevelFilter {
    match LevelFilter::from_str(lvl) {
        Ok(level) => level,
        Err(_) => {
            let default = LevelFilter::Error;
            println!("Unable to parse level string '{lvl}', defaulting to {default}");
            default
        }
    }
}

fn main() {
    let settings = Settings::parse_all_sources();

    // Build up the logging configuration such that:
    let config = ConfigBuilder::new()
        .set_time_level(LevelFilter::Off) // No timestamps are printed
        .set_location_level(LevelFilter::Off) // No file locations are printed
        .set_thread_level(LevelFilter::Off) // No thread information is printed
        .set_target_level(LevelFilter::Off) // No target is printed
        .build();
    SimpleLogger::init(choose_level(&settings.log.unwrap()), config).unwrap();

    let f = File::open(settings.bin_log_file.unwrap()).unwrap();
    let mut reader = BufReader::new(f);
    let mut checker = Checker::new(&settings.ids.unwrap());
    process_capnp(&mut reader, &mut checker);

    checker.end_checks();
}
