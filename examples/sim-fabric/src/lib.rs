// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a device comprising a rectangular fabric.
//!
//! The model is constructed the specified fabric and traffic generators
//! and sinks connected to all of the fabric ports.
//!
//! The traffic generators can be configured to send different traffic
//! patterns in order to evaluate the performance of the fabric.
//!
//! # Examples
//!
//! *Note*: in all the following comments the assumption is that a default
//! 1GHz clock is being used. If the default clock frequency were to be
//! changed then these calculations would be invalid.
//!
//! Running a basic all-to-all simulation
//! ```text
//! cargo run --bin sim-fabric --release -- --kib-to-send 1024 --stdout --traffic-pattern all-to-all-fixed
//! ```
//!
//! In order to achieve the maximum throughput it is essential to make the
//! frame sizes a multiple of the port width. For example, with a 128-bit
//! port and the 20-byte ethernet frame overhead an ideal frame size would
//! be something like 1484:
//! ```text
//! cargo run --bin sim-fabric --release -- --port-bits-per-tick 128 --frame-overhead-bytes 20 --frame-payload-bytes 1484 --kib-to-send 1024 --stdout
//! ```
//!
//! This achieves the peak bandwith at one port of 14.9 GiB/s and if run with
//! a balanced communcation pattern it can achieve that at each port (357.6
//! GiB/s for the default 24-port fabric):
//!
//! Note: A throughput of 342.70 GiB/s or less may be observed because the
//! src/dest       pairing is random and a source will not send to itself. Try a
//! different       value for `--seed` to observe this behaviour.
//!
//! ```text
//! cargo run --bin sim-fabric --release -- --port-bits-per-tick 128 --frame-overhead-bytes 20 --frame-payload-bytes 1484 --kib-to-send 1024 --traffic-pattern all-to-all-fixed --seed 3 --stdout
//! ```

pub mod frame_gen;
pub mod source_sink_builder;
