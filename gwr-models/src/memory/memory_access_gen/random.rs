// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::types::{AccessType, SimError};
use gwr_track::entity::Entity;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;

/// A Random address access generator.
///
/// Will emit memory accesses in the range [base, end)
pub struct Random {
    entity: Rc<Entity>,

    // For address -> device mapping
    memory_map: Rc<MemoryMap>,

    // Configuration
    src_addr: u64,
    base_addr: u64,
    addr_range: u64,
    alignment_mask: u64,
    overhead_size_bytes: usize,
    access_size_bytes: usize,
    num_to_send: usize,

    // State
    num_sent: usize,
    rng: StdRng,
}

impl Random {
    #[expect(clippy::too_many_arguments)]
    /// Construct a checked seeded stream without resampling invalid addresses.
    pub fn new(
        parent: &Rc<Entity>,
        name: &str,
        seed: u64,
        memory_map: &Rc<MemoryMap>,
        src_addr: u64,
        base_addr: u64,
        end_addr: u64,
        alignment_mask: u64,
        overhead_size_bytes: usize,
        access_size_bytes: usize,
        num_to_send: usize,
    ) -> Result<Self, SimError> {
        let alignment = (!alignment_mask)
            .checked_add(1)
            .filter(|a| a.is_power_of_two())
            .ok_or_else(|| SimError("Unsupported random alignment mask".into()))?;
        if end_addr <= base_addr {
            return Err(SimError(
                "Random generator requires a nonempty range".into(),
            ));
        }
        let first = base_addr & alignment_mask;
        let last = (end_addr - 1) & alignment_mask;
        let count = u128::from(last - first) / u128::from(alignment) + 1;
        super::validation::progression(memory_map, first, alignment, count, access_size_bytes)?;
        memory_map
            .resolve(src_addr)
            .ok_or_else(|| SimError("Unmapped generator reply address".into()))?;
        let rng = StdRng::seed_from_u64(seed);
        Ok(Self {
            entity: Rc::new(Entity::new(parent, name)),
            memory_map: memory_map.clone(),
            src_addr,
            base_addr,
            addr_range: end_addr - base_addr,
            alignment_mask,
            overhead_size_bytes,
            access_size_bytes,
            num_to_send,
            num_sent: 0,
            rng,
        })
    }
}

impl Iterator for Random {
    type Item = MemoryAccess;

    fn next(&mut self) -> Option<Self::Item> {
        if self.num_sent < self.num_to_send {
            self.num_sent += 1;

            let dst_addr =
                ((self.rng.next_u64() % self.addr_range) + self.base_addr) & self.alignment_mask;
            let resolved = self
                .memory_map
                .resolve_access(dst_addr, self.access_size_bytes)
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

            Some(access)
        } else {
            None
        }
    }
}
