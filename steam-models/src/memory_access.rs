// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fmt::Display;
use std::sync::Arc;

use steam_engine::traits::{Routable, SimObject, TotalBytes};
use steam_engine::types::{AccessType, SimError};
use steam_track::entity::Entity;
use steam_track::tag::Tagged;
use steam_track::{Tag, create_tag};

use crate::memory::{AccessMemory, CacheHintType, MemoryRead};

#[derive(Clone, Debug)]
pub struct MemoryAccess {
    created_by: Arc<Entity>,
    tag: Tag,
    access_type: AccessType,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    cache_hint: CacheHintType,
}

impl Display for MemoryAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}: {}@{:x}",
            self.access_type, self.num_bytes, self.dst_addr
        )
    }
}

impl TotalBytes for MemoryAccess {
    fn total_bytes(&self) -> usize {
        self.num_bytes
    }
}

impl Tagged for MemoryAccess {
    fn tag(&self) -> Tag {
        self.tag
    }
}

impl AccessMemory for MemoryAccess {
    fn access_type(&self) -> AccessType {
        self.access_type
    }

    fn dst_addr(&self) -> u64 {
        self.dst_addr
    }

    fn src_addr(&self) -> u64 {
        self.src_addr
    }

    fn cache_hint(&self) -> CacheHintType {
        CacheHintType::Allocate
    }

    fn num_bytes(&self) -> usize {
        self.num_bytes
    }

    fn to_response(&self, _mem: &impl MemoryRead) -> Self {
        MemoryAccess {
            created_by: self.created_by.clone(),
            tag: self.tag,
            access_type: AccessType::Write,
            num_bytes: self.num_bytes,
            dst_addr: self.src_addr,
            src_addr: self.dst_addr,
            cache_hint: self.cache_hint,
        }
    }
}

impl Routable for MemoryAccess {
    fn dest(&self) -> Result<u64, SimError> {
        Ok(self.dst_addr)
    }
    fn req_type(&self) -> Result<AccessType, SimError> {
        Ok(match self.access_type {
            AccessType::Read => AccessType::Read,
            AccessType::Write => AccessType::Write,
            AccessType::WriteNonPosted => AccessType::WriteNonPosted,
            AccessType::Control => AccessType::Control,
        })
    }
}

impl MemoryAccess {
    #[must_use]
    pub fn new(
        created_by: &Arc<Entity>,
        access_type: AccessType,
        num_bytes: usize,
        dst_addr: u64,
        src_addr: u64,
    ) -> Self {
        Self {
            created_by: created_by.clone(),
            tag: create_tag!(created_by),
            num_bytes,
            access_type,
            dst_addr,
            src_addr,
            cache_hint: CacheHintType::Allocate,
        }
    }
}

impl SimObject for MemoryAccess {}
