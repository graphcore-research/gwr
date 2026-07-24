// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fmt::Display;
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::{AccessType, SimError};
use gwr_track::entity::Entity;
use gwr_track::id::Unique;
use gwr_track::{Id, create_id, track_create_object};

use crate::cache::CacheHintType;
use crate::cache::coherency_manager::CoherenceOp;
use crate::cache::traits::CoherentAccess;
use crate::memory::memory_map::DeviceId;
use crate::memory::traits::{AccessMemory, ReadMemory};

#[derive(Clone, Debug)]
pub struct MemoryAccess {
    created_by: Rc<Entity>,
    id: Id,
    access_type: AccessType,
    access_size_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    dst_device: DeviceId,
    src_device: DeviceId,
    cache_hint: CacheHintType,
    coherence_op: Option<CoherenceOp>,

    /// Non-data overhead. Control/Read accesses don't contain any data.
    overhead_size_bytes: usize,
}

impl Display for MemoryAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} {}: {} bytes @ 0x{:x}. {} -> {}",
            self.access_type,
            self.id,
            self.access_size_bytes,
            self.dst_addr,
            self.src_device,
            self.dst_device,
        )
    }
}

impl TotalBytes for MemoryAccess {
    fn total_bytes(&self) -> usize {
        match self.access_type {
            AccessType::Control
            | AccessType::ReadRequest
            | AccessType::WriteNonPostedResponse
            | AccessType::BarrierRequest
            | AccessType::BarrierResponse => self.overhead_size_bytes,
            AccessType::WriteRequest
            | AccessType::WriteNonPostedRequest
            | AccessType::ReadResponse => self.access_size_bytes + self.overhead_size_bytes,
        }
    }
}

impl Unique for MemoryAccess {
    fn id(&self) -> Id {
        self.id
    }
}

impl AccessMemory for MemoryAccess {
    fn dst_addr(&self) -> u64 {
        self.dst_addr
    }

    fn src_addr(&self) -> u64 {
        self.src_addr
    }

    fn dst_device(&self) -> DeviceId {
        self.dst_device
    }

    fn src_device(&self) -> DeviceId {
        self.src_device
    }

    fn cache_hint(&self) -> CacheHintType {
        self.cache_hint
    }

    fn access_size_bytes(&self) -> usize {
        self.access_size_bytes
    }

    fn to_response(&self, _mem: &impl ReadMemory) -> Result<Self, SimError> {
        let response_type = match self.access_type {
            AccessType::Control => AccessType::Control,
            AccessType::ReadRequest => AccessType::ReadResponse,
            AccessType::WriteNonPostedRequest => AccessType::WriteNonPostedResponse,
            AccessType::BarrierRequest => AccessType::BarrierResponse,
            AccessType::ReadResponse
            | AccessType::WriteNonPostedResponse
            | AccessType::BarrierResponse
            | AccessType::WriteRequest => {
                return sim_error!("{}: unsupported by to_response()", self.access_type);
            }
        };
        Ok(MemoryAccess {
            created_by: self.created_by.clone(),
            id: self.id,
            access_type: response_type,
            access_size_bytes: self.access_size_bytes,
            dst_addr: self.dst_addr,
            src_addr: self.src_addr,
            dst_device: self.src_device,
            src_device: self.dst_device,
            cache_hint: self.cache_hint,
            coherence_op: self.coherence_op,
            overhead_size_bytes: self.overhead_size_bytes,
        })
    }
}

impl CoherentAccess for MemoryAccess {
    fn coherence_op(&self) -> Option<CoherenceOp> {
        self.coherence_op
    }

    fn with_access_type(mut self, access_type: AccessType) -> Self {
        self.access_type = access_type;
        self
    }

    fn with_dst_addr(mut self, dst_addr: u64) -> Self {
        self.dst_addr = dst_addr;
        self
    }

    fn with_coherence_op(mut self, coherence_op: Option<CoherenceOp>) -> Self {
        self.coherence_op = coherence_op;
        self
    }

    fn with_routing(mut self, dst_device: DeviceId, src_device: DeviceId) -> Self {
        self.dst_device = dst_device;
        self.src_device = src_device;
        self
    }
}

impl Routable for MemoryAccess {
    fn destination(&self) -> u64 {
        // The device ID is used for routing
        self.dst_device.0
    }
    fn access_type(&self) -> AccessType {
        self.access_type
    }
}

impl MemoryAccess {
    #[must_use]
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        created_by: &Rc<Entity>,
        access_type: AccessType,
        access_size_bytes: usize,
        dst_addr: u64,
        src_addr: u64,
        dst_device: DeviceId,
        src_device: DeviceId,
        overhead_size_bytes: usize,
    ) -> Self {
        let access = Self {
            created_by: created_by.clone(),
            id: create_id!(created_by),
            access_size_bytes,
            access_type,
            dst_addr,
            src_addr,
            dst_device,
            src_device,
            cache_hint: CacheHintType::Allocate,
            coherence_op: None,
            overhead_size_bytes,
        };
        track_create_object!(
            created_by;
            access.id,
            access.total_bytes(),
            "bytes",
            access.access_type() as u8,
            "MemoryAccess: {access}"
        );
        access
    }

    #[must_use]
    pub fn with_cache_hint(mut self, cache_hint: CacheHintType) -> Self {
        self.cache_hint = cache_hint;
        self
    }
}

impl SimObject for MemoryAccess {}
