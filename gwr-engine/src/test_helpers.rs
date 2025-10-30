// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use gwr_track::test_helpers::create_tracker;

use crate::engine::Engine;

#[must_use]
pub fn start_test(full_filepath: &str) -> Engine {
    Engine::new(&create_tracker(full_filepath))
}
