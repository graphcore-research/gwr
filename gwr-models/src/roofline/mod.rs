// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Shared planning primitives for the command-line analytical roofline mode.
//! These modules summarize physical traffic and lower it onto platform routes;
//! they do not register engine work or simulate events.

pub mod accounting;
pub mod plan;
pub mod traffic;
