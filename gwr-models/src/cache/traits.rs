// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::types::AccessType;

use crate::cache::coherency_manager::CoherenceOp;
use crate::memory::memory_map::DeviceId;
use crate::memory::traits::AccessMemory;

/// Trait implemented by memory access types that can participate in coherence.
pub trait CoherentAccess: AccessMemory + Clone {
    fn coherence_op(&self) -> Option<CoherenceOp>;
    fn with_access_type(self, access_type: AccessType) -> Self;
    fn with_dst_addr(self, dst_addr: u64) -> Self;
    fn with_coherence_op(self, coherence_op: Option<CoherenceOp>) -> Self;
    fn with_routing(self, dst_device: DeviceId, src_device: DeviceId) -> Self;
}
