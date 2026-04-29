// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Simulate a fast food restaurant.
//!
//! This is an example of how GWR can be used to simulate time-based,
//! event-driven systems. In this case, we choose to simulate a fast-
//! food restaurant with some tills and a kitchen in order understand
//! what might produce the most profitable staffing balance.
//!
//! This example simulates customers that arrive throughout the day. They decide
//! whether to join the queue, place orders and then wait, collect their food
//! and leave. It tracks costs vs income in order to understand the impact of
//! the staffing decisions on profitability.
//!
//! # Overview
//!
//! The code is structured into a library that models a restaurant and two
//! applications that use the library.
//!
//! The main files in the restaurant model are:
//!
//! - `src/lib/customer.rs` generates demand and tracks customer lifecycles.
//! - `src/lib/staff.rs` models till and kitchen workers as concurrent async
//!   processes.
//! - `src/lib/menu.rs` defines order templates, preparation time, and
//!   economics.
//!
//! The two applications using the library are:
//!
//! 1. The `sim-restaurant` is a command-line application that runs a sweep of
//!    staffing configurations in order to determine the most profitable.
//! 2. The `sim-restaurant-tui` allows the user to explore a single staffing
//!    configuration in more detail.
//!
//! # sim-restaurant
//!
//! The following command runs the restaurant simulation in order to sweep for
//! the most profitable staffing configuration. Running with the
//! `--help` argument will give full list of available configuration parameters.
//!
//! ```text
//! cargo run --bin sim-restaurant --release -- --max-till-staff 4 --max-kitchen-staff 5
//! ```
//!
//! # sim-restaurant-tui
//!
//! `sim-restaurant-tui` is useful when you want to explore a single staffing
//! configuration. For example:
//!
//! ```text
//! cargo run --bin sim-restaurant-tui --release -- --till-staff 2 --kitchen-staff 3
//! ```
//!
//! This replays one scenario letting you plot and dynamically explore:
//!
//! - Till and kitchen queue lengths.
//! - Busy till and kitchen workers.
//! - Profit vs costs.
//!
//! The controls are all detailed in the TUI window itself.

pub mod config;
mod customer;
mod menu;
pub mod recording;
pub mod sim;
mod staff;
pub mod time_of_day;
pub mod tui;

pub use staff::Staffing;
