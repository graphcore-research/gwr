// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Generate a self-contained browser report from GWR timetable data.

#![warn(missing_docs)]

mod address;
mod model;

#[cfg(feature = "generator")]
mod analysis;
#[cfg(feature = "generator")]
mod generator;
#[cfg(feature = "generator")]
pub use generator::{BundleInputs, write_bundle};
