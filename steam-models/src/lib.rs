// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Components that model higher-level functionality.
//!
//! Models will generally comprise one or more base
//! [components](steam_components) connected together with additional
//! functionality.

pub mod cache;
pub mod ethernet_frame;
pub mod ethernet_link;
pub mod fc_pipeline;
pub mod memory;
pub mod memory_access;
pub mod registers;
pub mod ring_node;
pub mod test_helpers;
