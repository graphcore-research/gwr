// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

pub mod arbiter;
pub mod cli;
pub mod connect;
pub mod delay;
pub mod flow_controls;
pub mod queue;
pub mod router;
pub mod sink;
pub mod source;
pub mod state_machine;
pub mod store;
pub mod test_helpers;
pub mod types;
