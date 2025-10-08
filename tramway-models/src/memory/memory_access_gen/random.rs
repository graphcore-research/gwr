// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::sync::Arc;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use tramway_engine::types::AccessType;
use tramway_track::entity::Entity;

use crate::memory::memory_access::MemoryAccess;

/// A Random address access generator.
///
/// Will emit memory accesses in the range [base, end)
pub struct Random {
    pub entity: Arc<Entity>,
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
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        parent: &Arc<Entity>,
        name: &str,
        seed: u64,
        src_addr: u64,
        base_addr: u64,
        end_addr: u64,
        alignment_mask: u64,
        overhead_size_bytes: usize,
        access_size_bytes: usize,
        num_to_send: usize,
    ) -> Self {
        let rng = StdRng::seed_from_u64(seed);
        Self {
            entity: Arc::new(Entity::new(parent, name)),
            src_addr,
            base_addr,
            addr_range: end_addr - base_addr,
            alignment_mask,
            overhead_size_bytes,
            access_size_bytes,
            num_to_send,
            num_sent: 0,
            rng,
        }
    }
}

impl Iterator for Random {
    type Item = MemoryAccess;
    fn next(&mut self) -> Option<Self::Item> {
        if self.num_sent < self.num_to_send {
            self.num_sent += 1;

            let dst_addr =
                ((self.rng.next_u64() % self.addr_range) + self.base_addr) & self.alignment_mask;
            let access = MemoryAccess::new(
                &self.entity,
                AccessType::Read,
                self.access_size_bytes,
                dst_addr,
                self.src_addr,
                self.overhead_size_bytes,
            );

            Some(access)
        } else {
            None
        }
    }
}
