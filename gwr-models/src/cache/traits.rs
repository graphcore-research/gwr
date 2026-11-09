// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_engine::types::{AccessType, DeviceId, SimError};

use crate::cache::coherency_manager::CoherenceOp;
use crate::memory::traits::{CacheAccess, CacheFillToken};

/// Directory-owned invalidation generation, independent of child fill identity.
#[derive(Clone, Debug)]
pub struct CoherenceToken {
    pub(crate) owner: Rc<()>,
    pub(crate) ordinal: u64,
}

impl CoherenceToken {
    pub(crate) fn matches(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.owner, &other.owner) && self.ordinal == other.ordinal
    }
}

/// Memory transactions that can participate in directory-managed cache
/// coherence.
///
/// Extends [`CacheAccess`] with coherence operations, explicit device routing,
/// and invalidation generation tokens. Ordinary caches and memories require
/// only their existing access traits; coherent caches and coherency managers
/// require this extension.
///
/// A packet may carry two independent identities: [`CacheFillToken`] identifies
/// a cache-owned child operation, while [`CoherenceToken`] identifies a
/// directory-owned invalidation round. Response construction through
/// [`AccessMemory::to_response`](crate::memory::traits::AccessMemory::to_response)
/// or [`AccessMemory::to_ack`](crate::memory::traits::AccessMemory::to_ack)
/// must preserve both tokens. Neither identity may be replaced by an address
/// or tracing ID: multiple operations can target the same line.
///
/// The `with_*` methods change only the named metadata, preserving the range,
/// allocation policy, and both identities unless explicitly replacing a token.
pub trait CoherentAccess: CacheAccess {
    /// Return the coherence operation carried by the packet.
    ///
    /// `None` denotes a packet without a coherence operation, such as a
    /// backing-memory transaction or barrier. The operation is separate from
    /// the base access type: invalidations and exclusive acquisitions can both
    /// use `AccessType::Control` with different coherence operations.
    fn coherence_op(&self) -> Option<CoherenceOp>;

    /// Return the packet with the supplied base access type.
    ///
    /// Preserve the coherence operation and routing. Byte accounting must
    /// reflect the new type, including whether it carries a data payload.
    fn with_access_type(self, access_type: AccessType) -> Self;

    /// Return the packet with the supplied coherence operation, or clear it
    /// with `None`. Preserve the base access type and routing.
    fn with_coherence_op(self, op: Option<CoherenceOp>) -> Self;

    /// Return the packet addressed to device `dst_device_id` from device
    /// `src_device_id`.
    ///
    /// Change device IDs only; preserve physical addresses and access size.
    /// This allows a child to travel through its manager to backing memory
    /// while retaining the identity needed to match its response.
    fn with_routing(self, dst_device_id: DeviceId, src_device_id: DeviceId) -> Self;

    /// Construct a child packet with the supplied access type, physical
    /// destination address, byte count, and cache-owned fill identity.
    ///
    /// Preserve allocation policy, device routing, source address, coherence
    /// operation, and invalidation token. Replace any existing fill token with
    /// `fill_token`; the original packet keeps its own identity for its
    /// eventual upstream response. The caller sets the child's operation
    /// and routing and clears any inherited invalidation token when
    /// starting unrelated work.
    ///
    /// # Errors
    ///
    /// Reject zero-sized or overflowing ranges and any access type or range
    /// the packet representation cannot support. The caller is responsible for
    /// coherence-line alignment and manager-domain checks.
    fn build_coherence_child_request(
        &self,
        access_type: AccessType,
        address: u64,
        bytes: usize,
        fill_token: CacheFillToken,
    ) -> Result<Self, SimError>
    where
        Self: Sized;

    /// Return the directory-owned invalidation generation token, if present.
    ///
    /// An invalidation acknowledgement must echo the invalidation's token so
    /// the directory can reject acknowledgements from stale or unrelated
    /// rounds. This token is independent of the packet's cache-owned fill
    /// identity.
    fn coherence_token(&self) -> Option<&CoherenceToken>;

    /// Return the packet with the supplied invalidation generation token, or
    /// clear it with `None`. Preserve the cache-owned fill token, operation,
    /// and routing. Echo the received token when building an acknowledgement;
    /// clear it when constructing a child unrelated to that invalidation round.
    fn with_coherence_token(self, invalidation_token: Option<CoherenceToken>) -> Self;
}
