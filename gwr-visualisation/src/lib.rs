// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Build report data from GWR timetable graphs.

#![warn(missing_docs)]
#![cfg_attr(not(test), allow(dead_code))]

mod address;
mod model;

#[cfg(feature = "generator")]
mod analysis;
