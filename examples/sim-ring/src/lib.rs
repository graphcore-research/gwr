// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Simulate a device comprising ring nodes.
//!
//! The model is constructed with as many ring nodes as specified by
//! the user. Each ring node will receive Ethernet Frames from a source
//! that should be routed all the way around the ring and end up at
//! the node effectively to its left.
//!
//! Limiters and flow control pipes are added to model actual hardware
//! limitations.
//!
//! The ring node contains an arbiter used to decide which frame to
//! grant next; the next ring frame or a new frame from the source.
//! The ring priority can be configured to demonstrate that incorrect
//! priority will lead to deadlock.
//!
//! # Examples
//!
//! Running a ring node that will lock up:
//! ```txt
//! cargo run --bin sim-ring --release -- --kib-to-send 1024 --stdout
//! ```
//!
//! But with increased ring priority the same model will pass:
//! ```txt
//! cargo run --bin sim-ring --release -- --kib-to-send 1024 --ring-priority 10 --stdout
//! ```
//!
//! # Diagram
//!
//! ```text
//!  /------------------------------------------------------------\
//!  |                                                            |
//!  |  +--------+                             +--------+         |
//!  |  | Source |                             | Source |         |
//!  |  +--------+                             +--------+         |
//!  |     |                                      |               |
//!  |     v                                      v               |
//!  |  +---------+                            +---------+        |
//!  |  | Limiter |                            | Limiter |        |
//!  |  +---------+                            +---------+        |
//!  |     |                                      |               |
//!  |     v                                      v               |
//!  |  +--------+                             +--------+         |
//!  |  | FcPipe |                             | FcPipe |         |
//!  |  +--------+                             +--------+         |
//!  |     |                                      |               |
//!  |     v                                      v               |
//!  |  +----------+  +---------+  +--------+  +----------+       |
//!  \->| RingNode |->| Limiter |->| FcPipe |->| RingNode | ...  -/
//!     +----------+  +---------+  +--------+  +----------+
//!        |                                      |
//!        v                                      v
//!     +---------+                            +---------+
//!     | Limiter |                            | Limiter |
//!     +---------+                            +---------+
//!        |                                      |
//!        v                                      v
//!     +------+                               +------+
//!     | Sink |                               | Sink |
//!     +------+                               +------+
//! ```

pub mod frame_gen;
pub mod ring_builder;
