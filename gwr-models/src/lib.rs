// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

#![doc = include_str!(gwr_build::generated_crate_docs_path!())]

use std::fmt::Display;
use std::rc::Rc;

use gwr_track::entity::Entity;
use gwr_track::info;

pub mod ethernet_frame;
pub mod ethernet_link;
pub mod fabric;
pub mod fc_pipeline;
pub mod memory;
pub mod processing_element;
pub mod registers;
pub mod ring_node;
pub mod test_helpers;

pub fn log_stats(entity: &Rc<Entity>, stats: impl Display) {
    for line in stats.to_string().lines() {
        info!(entity ; "{line}");
    }
}
