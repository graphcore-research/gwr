// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::fmt::Display;
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::traits::{Routable, SimObject, TotalBytes};
use gwr_engine::types::{AccessType, DeviceId, SimError};
use gwr_track::entity::Entity;
use gwr_track::id::Unique;
use gwr_track::{Id, create_id, track_create_object};

use crate::memory::CacheHintType;
use crate::memory::traits::{AccessMemory, CacheAccess, CacheFillToken, ReadMemory};

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
    cache_fill_token: Option<CacheFillToken>,

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
        if response_type == AccessType::ReadResponse {
            self.access_size_bytes
                .checked_add(self.overhead_size_bytes)
                .ok_or_else(|| SimError("read response byte size overflows usize".into()))?;
        }
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
            cache_fill_token: self.cache_fill_token.clone(),
            overhead_size_bytes: self.overhead_size_bytes,
        })
    }
}

impl CacheAccess for MemoryAccess {
    fn cache_hint(&self) -> CacheHintType {
        self.cache_hint
    }

    fn cache_read_fragment(
        &self,
        address: u64,
        bytes: usize,
        token: CacheFillToken,
    ) -> Result<Self, SimError> {
        if self.access_type != AccessType::ReadRequest
            || bytes == 0
            || address
                .checked_add(u64::try_from(bytes - 1).map_err(|e| SimError(e.to_string()))?)
                .is_none()
        {
            return Err(SimError("invalid cache fill range/type".into()));
        }
        let mut fragment = Self::new(
            &self.created_by,
            self.access_type,
            bytes,
            address,
            self.src_addr,
            self.dst_device,
            self.src_device,
            self.overhead_size_bytes,
        );
        fragment.cache_hint = self.cache_hint;
        fragment.cache_fill_token = Some(token);
        Ok(fragment)
    }

    fn cache_fill_token(&self) -> Option<&CacheFillToken> {
        self.cache_fill_token.as_ref()
    }
}

impl Routable for MemoryAccess {
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
            cache_fill_token: None,
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

    /// Explicit read allocation policy; defaults to Allocate. NoAllocate misses
    /// in the reference cache do not populate or coalesce allocating fills.
    #[must_use]
    pub fn with_cache_hint(mut self, hint: CacheHintType) -> Self {
        self.cache_hint = hint;
        self
    }
}

impl SimObject for MemoryAccess {}
