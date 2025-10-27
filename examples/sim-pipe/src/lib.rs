// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a flow-controlled pipeline.
//!
//! This allows the user to understand the performance impact on the pipeline
//! of different parameters like buffer size and credit delay.
//!
//! The simulation will create:
//! ```text
//!  Source -> Limiter -> FcPipeline -> Limiter -> Sink
//! ```
//!
//! # Examples
//!
//! Running a basic simulation
//! ```text
//! cargo run --bin sim-pipe --release -- --kib-to-send 1024 --stdout
//! ```
//!
//! # Impact of buffer size
//!
//! In order to see the impact of buffer size has on the throughput you can
//! run with simulations with different buffer sizes.
//!
//! If you use the default parameters you will get a pipeline which has a
//! bandwidth of 128-bits per cycle and frames that are 128-bit (8-byte header,
//! 8-byte payload). So the pipe will carry an entire frame every cycle.
//!
//! The default pipe has a 10-entry buffer and a latency in both directions
//! (data and credit) of 5 ticks. This results in a maximum data rate of the
//! 14.9GiB/s.
//! ```text
//! cargo run --bin sim-pipe --release -- --kib-to-send 1024 --stdout --progress
//! ```
//!
//! However, if you reduce the size of the buffer in the pipeline then you will
//! see the effective data rate reduce:
//! ```text
//! cargo run --bin sim-pipe --release -- --kib-to-send 1024 --stdout --progress --pipe-buffer-entries 9
//! ```
//!
//! In order to observe the bubbles in the pipeline you can use perfetto logs:
//! ```text
//! cargo run --bin sim-pipe --release -- --kib-to-send 1024 --stdout --progress --pipe-buffer-entries 9 --perfetto
//! ```
//! Then browse to <https://ui.perfetto.dev> and open the `trace.pftrace` file
//! that will have been generated. Within the `top::pipe::credit_pipe` row you
//! will see that it drops below the maximum value.

pub mod frame_gen;
