// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fmt::Display;
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::{AccessType, SimError};
use gwr_track::entity::Entity;
use gwr_track::id::Unique;
use gwr_track::{Id, create_id};

use crate::memory::CacheHintType;
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

    /// Non-data overhead. Control/Read accesses don't contain any data.
    overhead_size_bytes: usize,
}

impl Display for MemoryAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}: {}@{:x}",
            self.access_type, self.access_size_bytes, self.dst_addr
        )
    }
}

impl TotalBytes for MemoryAccess {
    fn total_bytes(&self) -> usize {
        match self.access_type {
            AccessType::Control | AccessType::ReadRequest | AccessType::WriteNonPostedResponse => {
                self.overhead_size_bytes
            }
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

    fn cache_hint(&self) -> CacheHintType {
        CacheHintType::Allocate
    }

    fn access_size_bytes(&self) -> usize {
        self.access_size_bytes
    }

    fn to_response(&self, _mem: &impl ReadMemory) -> Result<Self, SimError> {
        let response_type = match self.access_type {
            AccessType::Control => AccessType::Control,
            AccessType::ReadRequest => AccessType::ReadResponse,
            AccessType::WriteNonPostedRequest => AccessType::WriteNonPostedResponse,
            AccessType::ReadResponse
            | AccessType::WriteNonPostedResponse
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
            overhead_size_bytes: self.overhead_size_bytes,
        })
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
        Self {
            created_by: created_by.clone(),
            id: create_id!(created_by),
            access_size_bytes,
            access_type,
            dst_addr,
            src_addr,
            dst_device,
            src_device,
            cache_hint: CacheHintType::Allocate,
            overhead_size_bytes,
        }
    }
}

impl SimObject for MemoryAccess {}
