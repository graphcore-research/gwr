// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! Read and check a Cap'n Proto trace file created using steam_track
//!
//! Checks:
//!  - all tags used were created first
//!  - no destroyed tags are used again

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::str::FromStr;

use log::{LevelFilter, debug, error, info, trace, warn};
use simplelog::{ConfigBuilder, SimpleLogger};
use steam_config::multi_source_config;
use steam_track::Tag;
use steam_track::trace_visitor::{TraceVisitor, process_capnp};

/// Decide whether message is trace!() or info!()
///
/// The user can decide what messages are trace!() or info!() depending on what
/// tags are being displayed
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
/// tags are being displayed
macro_rules! info_debug {
    ($cond:expr, $($arg:tt)+) => (
        if $cond {
            info!($($arg)+);
        } else {
            debug!($($arg)+);
        }
    );
}

/// A tree structure to track which tags are created where
///
/// Simply store tags to connect to other Nodes which will be stored in a
/// separate HashMap
#[derive(Debug)]
struct Node {
    parent: Tag,
    tag: Tag,
    children: HashSet<Tag>,
}

impl Node {
    fn new(parent: Tag, tag: Tag) -> Self {
        Self {
            parent,
            tag,
            children: HashSet::new(),
        }
    }

    /// Add a child
    fn add_child(&mut self, child: Tag, log_as_info: bool) {
        self.children.insert(child);
        info_debug!(log_as_info, "{} added child {}", self.tag, child);
    }

    /// Remove a child and return whether the children are now empty
    fn remove_child(&mut self, child: Tag, log_as_info: bool) -> bool {
        if self.children.remove(&child) {
            info_debug!(log_as_info, "{} removed child {}", self.tag, child);
        } else {
            error!(
                "{} attempting to remove child {} not in children",
                self.tag, child
            );
        }

        self.children.is_empty()
    }
}

/// The Checker is responsible for keeping track of all state and checking its
/// correctness
struct Checker {
    active_tags: HashMap<Tag, Node>,
    destroyed: HashMap<Tag, Node>,
    info_tags: HashSet<Tag>,
}

impl Checker {
    fn new(info_tags: &[u64]) -> Self {
        let mut checker = Self {
            active_tags: HashMap::new(),
            destroyed: HashMap::new(),
            info_tags: HashSet::new(),
        };

        for tag in info_tags {
            checker.info_tags.insert(Tag(*tag));
        }

        let root = Node::new(steam_track::NO_ID, steam_track::ROOT);
        let no_id = Node::new(steam_track::NO_ID, steam_track::NO_ID);

        // It is always ok to use these special identifiers from steam_track
        checker.active_tags.insert(steam_track::ROOT, root);
        checker.active_tags.insert(steam_track::NO_ID, no_id);

        checker
    }

    fn check(&self, tag: Tag) {
        if !self.active_tags.contains_key(&tag) {
            error!("Location tag {tag} used when not active");
        }
    }

    fn create_tag(&mut self, tag: Tag, created_by: Tag) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.active_tags.entry(tag) {
            e.insert(Node::new(created_by, tag));
            self.add_child(created_by, tag);
        } else {
            error!("Attempting to create existing tag {tag}");
        }
    }

    fn destroy_tag(&mut self, tag: Tag) {
        if tag == steam_track::ROOT || tag == steam_track::NO_ID {
            error!("Unable to destroy {tag}");
            return;
        }

        if let Some(node) = self.active_tags.remove(&tag) {
            self.remove_child(node.parent, tag);

            if !node.children.is_empty() {
                info!("attempting to destroy obj {tag} with active children");
                self.destroyed.insert(tag, node);
            } else {
                info_debug!(self.info_tags.contains(&tag), "{} destroyed", tag);
            }
        } else {
            error!("attempting to destroy unknown tag {tag}");
        }
    }

    fn add_child(&mut self, parent: Tag, child: Tag) {
        let log_as_info = self.log_as_info(child);
        if let Some(node) = self.active_tags.get_mut(&parent) {
            node.add_child(child, log_as_info);
        } else {
            error!("attempting to add {child} to unknown parent {parent}");
        }
    }

    fn remove_child(&mut self, parent: Tag, child: Tag) {
        let log_as_info = self.log_as_info(child);
        if let Some(parent_node) = self.active_tags.get_mut(&parent) {
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

    fn log_as_info(&self, tag: Tag) -> bool {
        self.info_tags.contains(&tag)
    }

    fn end_checks(&self) {
        let still_active = self
            .active_tags
            .iter()
            .filter(|&(k, _v)| (*k != steam_track::NO_ID && *k != steam_track::ROOT));

        for (k, v) in still_active {
            warn!("Tag {k} still active");
            if !v.children.is_empty() {
                warn!("  with children {:?}", v.children);
            }
        }
    }
}

impl TraceVisitor for Checker {
    fn log(&mut self, tag: Tag, level: log::Level, message: &str) {
        info_trace!(
            self.log_as_info(tag),
            "{}:{}: Message: {}",
            tag,
            level,
            message
        );

        self.check(tag);
    }

    fn create(&mut self, created_by: Tag, tag: Tag, num_bytes: usize, req_type: i8, name: &str) {
        info_trace!(
            self.log_as_info(tag),
            "{}: created {}, {}, {}, {} bytes",
            created_by,
            tag,
            name,
            req_type,
            num_bytes,
        );

        self.create_tag(tag, created_by);
        self.check(tag);
    }

    fn destroy(&mut self, tag: Tag, destroyed_by: Tag) {
        info_trace!(self.log_as_info(tag), "{}: destroyed {}", destroyed_by, tag,);
        self.check(tag);
        self.destroy_tag(tag);
    }

    fn enter(&mut self, tag: Tag, entered: Tag) {
        info_trace!(self.log_as_info(tag), "{}: {} enter", tag, entered);

        self.check(tag);
    }

    fn exit(&mut self, tag: Tag, exited: Tag) {
        info_trace!(self.log_as_info(tag), "{}: {} exit", tag, exited);

        self.check(tag);
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

    /// Tag IDs to log at INFO level
    #[arg(long = "tags")]
    tags: Option<Vec<u64>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            log: Some("info".to_string()),
            bin_log_file: Some(Default::default()),
            tags: Some(Default::default()),
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
    let mut checker = Checker::new(&settings.tags.unwrap());
    process_capnp(&mut reader, &mut checker);

    checker.end_checks();
}
