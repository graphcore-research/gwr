// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use gwr_engine::traits::{Routable, TotalBytes};
use gwr_engine::types::SimError;

use crate::cache::CacheHintType;
use crate::memory::memory_map::DeviceId;

pub trait ReadMemory {
    fn read(&self) -> Vec<u8>;
}

impl ReadMemory for () {
    fn read(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// Trait implemented by all types that memory components support
pub trait AccessMemory
where
    Self: Routable + TotalBytes,
{
    /// Return the destination address of this access
    fn dst_addr(&self) -> u64;

    /// Return the source address of this access
    fn src_addr(&self) -> u64;

    /// Return the destination device of this access
    fn dst_device(&self) -> DeviceId;

    /// Return the source device of this access
    fn src_device(&self) -> DeviceId;

    /// Return the size of the access in bytes
    fn access_size_bytes(&self) -> usize;

    /// Returns the appropriate response for a request
    fn to_response(&self, mem: &impl ReadMemory) -> Result<Self, SimError>
    where
        Self: Sized;

    /// Returns the requested caching behaviour of a request
    fn cache_hint(&self) -> CacheHintType;
}
