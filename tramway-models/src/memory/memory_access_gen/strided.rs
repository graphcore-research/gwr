// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use tramway_engine::types::AccessType;
use tramway_track::entity::Entity;

use crate::memory::memory_access::MemoryAccess;

/// A Strided address access generator.
///
/// Will emit memory accesses in the range [base, end)
pub struct Strided {
    pub entity: Rc<Entity>,
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
    #[must_use]
    pub fn new(
        parent: &Rc<Entity>,
        name: &str,
        src_addr: u64,
        base_addr: u64,
        end_addr: u64,
        stride_bytes: u64,
        overhead_size_bytes: usize,
        access_size_bytes: usize,
        num_to_send: usize,
    ) -> Self {
        Self {
            entity: Rc::new(Entity::new(parent, name)),
            src_addr,
            base_addr,
            end_addr,
            stride_bytes,
            overhead_size_bytes,
            access_size_bytes,
            num_to_send,
            next_addr: base_addr,
            num_sent: 0,
        }
    }
}

impl Iterator for Strided {
    type Item = MemoryAccess;
    fn next(&mut self) -> Option<Self::Item> {
        if self.num_sent < self.num_to_send {
            self.num_sent += 1;

            let access = MemoryAccess::new(
                &self.entity,
                AccessType::Read,
                self.access_size_bytes,
                self.next_addr,
                self.src_addr,
                self.overhead_size_bytes,
            );

            self.next_addr += self.stride_bytes;
            if self.next_addr >= self.end_addr {
                self.next_addr = self.base_addr;
            }

            Some(access)
        } else {
            None
        }
    }
}
