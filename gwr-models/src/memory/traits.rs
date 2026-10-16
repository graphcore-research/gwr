// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::traits::{Routable, TotalBytes};
use gwr_engine::types::SimError;

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

    /// Returns the appropriate response for a request
    fn to_response(&self, mem: &impl ReadMemory) -> Result<Self, SimError>
    where
        Self: Sized;
}

/// Memory transactions that can pass through a cache.
///
/// Original requests may have no fill token. Each internal read fragment must
/// carry the supplied token, and [`AccessMemory::to_response`] must preserve
/// it. When caches are nested, each cache creates tokens for its own fills
/// while retaining the original request's token for the response it returns
/// upstream.
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
