// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Generate a self-contained browser report from GWR timetable data.

#![warn(missing_docs)]

mod model;
mod payload;

#[cfg(feature = "generator")]
mod analysis;
#[cfg(feature = "generator")]
mod generator;
#[cfg(feature = "generator")]
pub use generator::{BundleInputs, write_bundle};

#[cfg(any(all(feature = "web", target_arch = "wasm32"), test))]
#[cfg_attr(not(all(feature = "web", target_arch = "wasm32")), allow(dead_code))]
mod web;
