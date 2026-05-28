// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Coherent packet operations, isolated from the ordinary CacheAccess contract.
use std::rc::Rc;

use gwr_engine::types::{AccessType, DeviceId, SimError, SimResult};

use crate::cache::coherency_manager::CoherenceOp;
use crate::cache::traits::CoherentAccess;
use crate::memory::memory_map::MemoryMap;
use crate::memory::traits::CacheFillToken;

#[derive(Clone)]
pub struct CoherenceConfig {
    pub(crate) device_id: DeviceId,
    pub(crate) manager_map: Rc<MemoryMap>,
}

impl CoherenceConfig {
    #[must_use]
    pub fn new(device_id: DeviceId, manager_map: Rc<MemoryMap>) -> Self {
        Self {
            device_id,
            manager_map,
        }
    }

    pub(super) fn validate(&self, line: usize) -> SimResult {
        if line == 0 || self.manager_map.num_regions() == 0 {
            return Err(SimError(
                "coherence requires a positive line size and nonempty manager map".into(),
            ));
        }
        for region in self.manager_map.regions() {
            if region.device == self.device_id {
                return Err(SimError("cache cannot be its own coherency manager".into()));
            }
            if !(region.start as u128).is_multiple_of(line as u128)
                || !(region.end as u128 + 1).is_multiple_of(line as u128)
            {
                return Err(SimError(
                    "coherency manager map must align to cache lines".into(),
                ));
            }
        }
        Ok(())
    }

    fn manager_device_for_range(&self, address: u64, bytes: usize) -> Result<DeviceId, SimError> {
        let last = address
            .checked_add(u64::try_from(bytes.max(1) - 1).map_err(|e| SimError(e.to_string()))?)
            .ok_or_else(|| SimError("coherence range overflows".into()))?;
        let (manager_device_id, offset) = self
            .manager_map
            .lookup(address)
            .ok_or_else(|| SimError(format!("no coherency manager at {address:#x}")))?;
        let end = self.manager_map.lookup(last);
        if end != Some((manager_device_id, offset + (last - address))) {
            return Err(SimError("coherence range crosses manager boundary".into()));
        }
        Ok(manager_device_id)
    }
}

/// Packet inspection and construction needed by the optional coherence path.
/// Keeps the cache core usable with packets implementing only `CacheAccess`.
pub(super) trait CoherenceProtocol<T> {
    /// Return the packet's coherence operation, or `None` for ordinary traffic
    /// such as backing-memory transactions and barriers.
    fn coherence_op(&self, packet: &T) -> Option<CoherenceOp>;

    /// Construct a child packet for the supplied access type, byte range,
    /// fill identity, and coherence operation, preserving the allocation
    /// policy. Clear any inherited invalidation token and route from this
    /// cache to the manager responsible for the entire range. Fail if child
    /// construction fails or the range is unmapped, overflows, or crosses a
    /// manager boundary.
    fn build_child_request(
        &self,
        packet: &T,
        kind: AccessType,
        address: u64,
        bytes: usize,
        fill_token: CacheFillToken,
        op: Option<CoherenceOp>,
    ) -> Result<T, SimError>;

    /// Acknowledge a validated invalidation to its sending manager. Preserve
    /// the line range, child identity, and invalidation generation token, while
    /// setting the access type to `Control` and operation to `InvalidateAck`.
    fn build_invalidation_ack(&self, packet: &T) -> T;

    /// Construct a barrier child with the supplied fill identity. Use a
    /// one-byte range at the original address to select its manager, clear
    /// coherence operation and invalidation token, and route from this cache.
    /// Fail if child construction or manager lookup fails.
    fn build_barrier_request(&self, packet: &T, fill_token: CacheFillToken) -> Result<T, SimError>;

    /// Check that an invalidation is a control packet carrying a generation
    /// token, covers one aligned `line`-byte range, and is addressed to this
    /// cache by the manager responsible for that range. Fail on malformed
    /// metadata or an invalid manager mapping. The caller checks the operation;
    /// this check does not establish whether the generation token is current.
    fn validate_invalidation(&self, packet: &T, line: usize) -> SimResult;
}

/// Cache-side packet operations for directory-managed coherence.
pub(super) struct DirectoryCoherenceProtocol(pub CoherenceConfig);

impl<T: CoherentAccess + Clone> CoherenceProtocol<T> for DirectoryCoherenceProtocol {
    fn coherence_op(&self, packet: &T) -> Option<CoherenceOp> {
        packet.coherence_op()
    }

    fn build_child_request(
        &self,
        packet: &T,
        kind: AccessType,
        address: u64,
        bytes: usize,
        fill_token: CacheFillToken,
        op: Option<CoherenceOp>,
    ) -> Result<T, SimError> {
        Ok(packet
            .build_coherence_child_request(kind, address, bytes, fill_token)?
            .with_coherence_op(op)
            .with_coherence_token(None)
            .with_routing(
                self.0.manager_device_for_range(address, bytes)?,
                self.0.device_id,
            ))
    }

    fn build_invalidation_ack(&self, packet: &T) -> T {
        packet
            .clone()
            .with_access_type(AccessType::Control)
            .with_coherence_op(Some(CoherenceOp::InvalidateAck))
            .with_routing(packet.src_device(), self.0.device_id)
    }

    fn build_barrier_request(&self, packet: &T, fill_token: CacheFillToken) -> Result<T, SimError> {
        Ok(packet
            .build_coherence_child_request(
                AccessType::BarrierRequest,
                packet.dst_addr(),
                1,
                fill_token,
            )?
            .with_coherence_op(None)
            .with_coherence_token(None)
            .with_routing(
                self.0.manager_device_for_range(packet.dst_addr(), 1)?,
                self.0.device_id,
            ))
    }

    fn validate_invalidation(&self, packet: &T, line: usize) -> SimResult {
        if packet.access_type() != AccessType::Control
            || packet.coherence_token().is_none()
            || !packet.dst_addr().is_multiple_of(line as u64)
            || packet.access_size_bytes() != line
            || packet.src_device() != self.0.manager_device_for_range(packet.dst_addr(), line)?
            || packet.dst_device() != self.0.device_id
        {
            return Err(SimError("malformed coherence invalidation".into()));
        }
        Ok(())
    }
}
