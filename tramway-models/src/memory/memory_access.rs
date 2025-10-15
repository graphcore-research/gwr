// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fmt::Display;
use std::rc::Rc;

use tramway_engine::traits::{Routable, SimObject, TotalBytes};
use tramway_engine::types::AccessType;
use tramway_track::entity::Entity;
use tramway_track::id::Unique;
use tramway_track::{Id, create_id};

use crate::memory::CacheHintType;
use crate::memory::traits::{AccessMemory, ReadMemory};

#[derive(Clone, Debug)]
pub struct MemoryAccess {
    created_by: Rc<Entity>,
    id: Id,
    access_type: AccessType,
    access_size_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
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
            AccessType::Control | AccessType::Read => self.overhead_size_bytes,
            AccessType::Write | AccessType::WriteNonPosted => {
                self.access_size_bytes + self.overhead_size_bytes
            }
        }
    }
}

impl Unique for MemoryAccess {
    fn id(&self) -> Id {
        self.id
    }
}

impl AccessMemory for MemoryAccess {
    fn source(&self) -> u64 {
        self.src_addr
    }

    fn cache_hint(&self) -> CacheHintType {
        CacheHintType::Allocate
    }

    fn access_size_bytes(&self) -> usize {
        self.access_size_bytes
    }

    fn to_response(&self, _mem: &impl ReadMemory) -> Self {
        MemoryAccess {
            created_by: self.created_by.clone(),
            id: self.id,
            access_type: AccessType::Write,
            access_size_bytes: self.access_size_bytes,
            dst_addr: self.src_addr,
            src_addr: self.dst_addr,
            cache_hint: self.cache_hint,
            overhead_size_bytes: self.overhead_size_bytes,
        }
    }
}

impl Routable for MemoryAccess {
    fn destination(&self) -> u64 {
        self.dst_addr
    }
    fn access_type(&self) -> AccessType {
        self.access_type
    }
}

impl MemoryAccess {
    #[must_use]
    pub fn new(
        created_by: &Rc<Entity>,
        access_type: AccessType,
        access_size_bytes: usize,
        dst_addr: u64,
        src_addr: u64,
        overhead_size_bytes: usize,
    ) -> Self {
        Self {
            created_by: created_by.clone(),
            id: create_id!(created_by),
            access_size_bytes,
            access_type,
            dst_addr,
            src_addr,
            cache_hint: CacheHintType::Allocate,
            overhead_size_bytes,
        }
    }
}

impl SimObject for MemoryAccess {}
