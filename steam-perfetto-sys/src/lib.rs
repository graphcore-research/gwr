// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! This crate is provided to wrap up the build of Perfetto.
//!
//! # Features
//!
//! By default this crate will not download or build Perfetto. The following
//! features can be enabled to cause it to do so:
//!
//! - `_perfetto_src` - Download the Perfetto source code and symlink it into
//!   the source tree of this crate.
//! - `_perfetto_ui` - Build the Perfetto UI from the downloaded source (implies
//!   `_perfetto_src`).
