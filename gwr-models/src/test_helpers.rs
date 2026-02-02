// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::types::AccessType;
use gwr_track::entity::Entity;

use crate::memory::memory_access::MemoryAccess;

#[must_use]
pub fn create_read(
    created_by: &Rc<Entity>,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    overhead_size_bytes: usize,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::ReadRequest,
        num_bytes,
        dst_addr,
        src_addr,
        overhead_size_bytes,
    )
}

#[must_use]
pub fn create_write(
    created_by: &Rc<Entity>,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    overhead_size_bytes: usize,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::WriteRequest,
        num_bytes,
        dst_addr,
        src_addr,
        overhead_size_bytes,
    )
}

#[must_use]
pub fn create_write_np(
    created_by: &Rc<Entity>,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    overhead_size_bytes: usize,
) -> MemoryAccess {
    MemoryAccess::new(
        created_by,
        AccessType::WriteNonPostedRequest,
        num_bytes,
        dst_addr,
        src_addr,
        overhead_size_bytes,
    )
}
