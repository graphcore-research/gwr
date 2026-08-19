// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

/// Application.
pub mod app;

mod bin_loader;
mod filter;
mod log_parser;
mod renderer;

/// Terminal events handler.
pub mod event;

/// Widget renderer.
pub mod ui;

/// Terminal user interface.
pub mod tui;

/// Event handler.
pub mod handler;

pub mod rocket;

#[cfg(test)]
#[path = "../tests/common/server_contract.rs"]
mod server_contract;

#[cfg(test)]
pub(crate) use rocket::SHARED_STATE as TEST_SERVER_STATE;

/// Perfetto output generator.
#[cfg(feature = "perfetto")]
pub mod perfetto;
