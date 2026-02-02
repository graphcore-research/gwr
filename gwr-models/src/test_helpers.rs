// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::types::AccessType;
use gwr_track::entity::Entity;

use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::{DeviceId, MemoryMap};

#[must_use]
pub fn create_default_memory_map() -> MemoryMap {
    let mut memory_map = MemoryMap::new();

    // Map all addresses to a single device ID.
    memory_map.insert(0x0, u64::MAX, DeviceId(0)).unwrap();

    memory_map
}

#[must_use]
pub fn create_read(
    created_by: &Rc<Entity>,
    memory_map: &Rc<MemoryMap>,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    overhead_size_bytes: usize,
) -> MemoryAccess {
    let (dst_device, _) = memory_map.lookup(dst_addr).unwrap();
    let (src_device, _) = memory_map.lookup(src_addr).unwrap();
    MemoryAccess::new(
        created_by,
        AccessType::ReadRequest,
        num_bytes,
        dst_addr,
        src_addr,
        dst_device,
        src_device,
        overhead_size_bytes,
    )
}

#[must_use]
pub fn create_write(
    created_by: &Rc<Entity>,
    memory_map: &Rc<MemoryMap>,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    overhead_size_bytes: usize,
) -> MemoryAccess {
    let (dst_device, _) = memory_map.lookup(dst_addr).unwrap();
    let (src_device, _) = memory_map.lookup(src_addr).unwrap();
    MemoryAccess::new(
        created_by,
        AccessType::WriteRequest,
        num_bytes,
        dst_addr,
        src_addr,
        dst_device,
        src_device,
        overhead_size_bytes,
    )
}

#[must_use]
pub fn create_write_np(
    created_by: &Rc<Entity>,
    memory_map: &Rc<MemoryMap>,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    overhead_size_bytes: usize,
) -> MemoryAccess {
    let (dst_device, _) = memory_map.lookup(dst_addr).unwrap();
    let (src_device, _) = memory_map.lookup(src_addr).unwrap();
    MemoryAccess::new(
        created_by,
        AccessType::WriteNonPostedRequest,
        num_bytes,
        dst_addr,
        src_addr,
        dst_device,
        src_device,
        overhead_size_bytes,
    )
}
