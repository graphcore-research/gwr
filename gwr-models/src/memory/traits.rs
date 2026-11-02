// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::traits::{Routable, TotalBytes};
use gwr_engine::types::{DeviceId, SimError, SimResult};

use crate::memory::CacheHintType;

/// Component-owned fill identity, independent of optional tracing IDs.
/// The response must preserve this token; nested caches retain their original
/// parent token while using a distinct token for their own child transaction.
#[derive(Clone, Debug)]
pub struct CacheFillToken {
    pub(crate) owner: Rc<()>,
    pub(crate) ordinal: u64,
}

pub trait ReadMemory {
    fn read(&self) -> Vec<u8>;
}

/// Trait implemented by all types that memory components support
pub trait AccessMemory
where
    Self: Routable + TotalBytes,
{
    /// Return the size of the access in bytes
    fn access_size_bytes(&self) -> usize;

    /// Construct the appropriate response, using `mem` when read data is
    /// required. Preserve the request range and transaction identities and
    /// reverse device routing. For requests without response data, callers
    /// can use [`Self::to_ack`].
    fn to_response(&self, mem: &impl ReadMemory) -> Result<Self, SimError>
    where
        Self: Sized;

    /// Construct a header-only acknowledgement without accessing memory data.
    ///
    /// Acknowledge control, nonposted-write, and barrier requests, preserving
    /// their range, allocation policy, and transaction identities and
    /// reversing device routing. The access size still describes the
    /// acknowledged range; it must not be counted as response payload.
    ///
    /// # Errors
    ///
    /// Reject reads, which require the memory-backed response path, posted
    /// writes, which have no response, and packets that are already
    /// responses.
    fn to_ack(&self) -> Result<Self, SimError>
    where
        Self: Sized;
}

/// Traffic-model transactions that can be split by a memory striper.
///
/// The striper owns address translation, fragment boundaries, routing and
/// completion tracking. Packet implementations own wire-byte accounting and
/// protocol metadata, including fields unknown to the striper.
///
/// Fragment responses must echo the supplied fragment ID in
/// [`Routable::src_addr`], independently of optional tracing identities.
/// Once all fragments of a read or nonposted write complete, the striper
/// constructs one response from the original request. Reads use
/// [`AccessMemory::to_response`] with a [`ReadMemory`] that returns no data;
/// nonposted writes use [`AccessMemory::to_ack`]. Posted writes produce no
/// response. Implementations must support this traffic-only completion path;
/// fragment payloads are not assembled.
pub trait StripingAccess: AccessMemory {
    /// Construct a fragment with the supplied translated range and device
    /// routing, setting its source address to `fragment_id`.
    ///
    /// Preserve the parent's access type and all other protocol metadata.
    /// Support reads, posted writes and nonposted writes; reject unsupported,
    /// empty or overflowing ranges. Create a fresh tracing identity when
    /// tracking is enabled, while retaining the parent's identity for its
    /// eventual upstream response.
    fn stripe_fragment(
        &self,
        address: u64,
        bytes: usize,
        destination: DeviceId,
        source: DeviceId,
        fragment_id: u64,
    ) -> Result<Self, SimError>
    where
        Self: Sized;

    /// Validate a response against this retained fragment request.
    ///
    /// The striper has already checked the response type, device routing,
    /// translated address, byte count and fragment ID. Check the packet's
    /// wire-byte accounting and protocol metadata, returning a descriptive
    /// error for any mismatch without changing either packet.
    fn validate_stripe_response(&self, response: &Self) -> SimResult;
}

/// Memory transactions that can pass through a cache.
///
/// Original requests may have no fill token. Each internal read fragment must
/// carry the supplied token. Both [`AccessMemory::to_response`] and
/// [`AccessMemory::to_ack`] must preserve it. When caches are
/// nested, each cache creates tokens for its own fills while retaining the
/// original request's token for the response it returns upstream.
pub trait CacheAccess: AccessMemory {
    /// Return the allocation policy requested by this transaction.
    fn cache_hint(&self) -> CacheHintType;

    /// Create an internal read for the given physical address and byte count.
    /// Preserve routing information and allocation policy, and attach `token`.
    /// Reject unsupported ranges or transaction types with an error.
    fn cache_read_fragment(
        &self,
        address: u64,
        bytes: usize,
        token: CacheFillToken,
    ) -> Result<Self, SimError>
    where
        Self: Sized;

    /// Return the fill identity carried by this transaction, if any.
    /// A fragment and its response must carry the same token.
    fn cache_fill_token(&self) -> Option<&CacheFillToken>;
}
