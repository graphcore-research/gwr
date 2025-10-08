// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::fs;
use std::io::BufWriter;
use std::path::Path;
use std::sync::Arc;

use tramway_track::tracker::{CapnProtoTracker, EntityManager};
use tramway_track::{Tracker, Writer};

use crate::engine::Engine;

#[must_use]
pub fn create_tracker(full_filepath: &str) -> Tracker {
    // Place all trace files in one folder
    const FOLDER: &str = "traces";

    // Create that folder if it doesn't exist yet
    fs::create_dir_all(FOLDER).unwrap();

    let filename_only = Path::new(full_filepath)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap();

    let bin_writer: Writer = Box::new(BufWriter::new(
        fs::File::create(format!("{FOLDER}/{filename_only}.bin")).unwrap(),
    ));

    let default_log_level = log::Level::Trace;
    let entity_manger = EntityManager::new(default_log_level);
    let tracker: Tracker = Arc::new(CapnProtoTracker::new(entity_manger, bin_writer));
    tracker
}

#[must_use]
pub fn start_test(full_filepath: &str) -> Engine {
    Engine::new(&create_tracker(full_filepath))
}
