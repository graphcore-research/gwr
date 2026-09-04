// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

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
