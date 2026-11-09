// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Bounded direct-port atomic/control memory foundation.
mod contract;
mod port_memory;
mod service;
pub use contract::*;
pub use port_memory::{AtomicControlHandle, AtomicMemory, AtomicPacket, AtomicReply};
pub use service::{AtomicService, AtomicSnapshot, Completion, DebugState, Ticket};
