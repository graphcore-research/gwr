// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::types::{AccessType, SimError};
use gwr_track::entity::Entity;

use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;

/// A Strided address access generator.
///
/// Will emit memory accesses in the range [base, end)
pub struct Strided {
    entity: Rc<Entity>,
    memory_map: Rc<MemoryMap>,

    // Configuration
    src_addr: u64,
    base_addr: u64,
    end_addr: u64,
    stride_bytes: u64,
    overhead_size_bytes: usize,
    access_size_bytes: usize,
    num_to_send: usize,

    // State
    next_addr: u64,
    num_sent: usize,
}

impl Strided {
    #[expect(clippy::too_many_arguments)]
    /// Construct a checked finite stream; invalid packet geometry fails before
    /// issue.
    pub fn new(
        parent: &Rc<Entity>,
        name: &str,
        memory_map: &Rc<MemoryMap>,
        src_addr: u64,
        base_addr: u64,
        end_addr: u64,
        stride_bytes: u64,
        overhead_size_bytes: usize,
        access_size_bytes: usize,
        num_to_send: usize,
    ) -> Result<Self, SimError> {
        if end_addr <= base_addr || stride_bytes == 0 {
            return Err(SimError(
                "Generator requires a nonempty range and nonzero stride".into(),
            ));
        }
        let cycle = u128::from(end_addr - base_addr).div_ceil(u128::from(stride_bytes));
        super::validation::progression(
            memory_map,
            base_addr,
            stride_bytes,
            cycle.min(num_to_send as u128),
            access_size_bytes,
        )?;
        memory_map
            .resolve(src_addr)
            .ok_or_else(|| SimError("Unmapped generator reply address".into()))?;
        Ok(Self {
            entity: Rc::new(Entity::new(parent, name)),
            memory_map: memory_map.clone(),
            src_addr,
            base_addr,
            end_addr,
            stride_bytes,
            overhead_size_bytes,
            access_size_bytes,
            num_to_send,
            next_addr: base_addr,
            num_sent: 0,
        })
    }
}

impl Iterator for Strided {
    type Item = MemoryAccess;
    fn next(&mut self) -> Option<Self::Item> {
        if self.num_sent < self.num_to_send {
            self.num_sent += 1;

            let resolved = self
                .memory_map
                .resolve_access(self.next_addr, self.access_size_bytes)
                .expect("validated generator geometry");
            let dst_device = resolved.device;
            let (src_device, _) = self.memory_map.lookup(self.src_addr)?;

            let access = MemoryAccess::new(
                &self.entity,
                AccessType::ReadRequest,
                self.access_size_bytes,
                resolved.physical_address,
                self.src_addr,
                dst_device,
                src_device,
                self.overhead_size_bytes,
            );

            self.next_addr = self
                .next_addr
                .checked_add(self.stride_bytes)
                .filter(|&next| next < self.end_addr)
                .unwrap_or(self.base_addr);

            Some(access)
        } else {
            None
        }
    }
}
