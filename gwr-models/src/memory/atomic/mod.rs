// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Explicit direct-port atomic/control memory foundation. Ordinary Memory is
//! unchanged. No AMD adapter, numeric BF16 implementation or cache bypass.
mod contract;
mod port_memory;
mod service;
pub use contract::*;
pub use port_memory::{AtomicControlHandle, AtomicMemory, AtomicPacket, AtomicReply};
pub use service::{AtomicService, AtomicSnapshot, Completion, DebugState, Ticket};

mod shared;
pub(crate) use shared::SharedMemoryAuthority;
pub use shared::{AtomicOwnerWindow, AtomicReceipt, AtomicTargetSender, SharedAtomicControl};

mod controller;
pub use controller::{ControllerObservation, ParticipantObservation, SplitKController};
