// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Control and Status Register Fields.

use crate::registers::Permission;

#[derive(Clone)]
pub struct Field {
    offset: usize,
    num_bits: usize,
    reset_value: u64,
}

impl Field {
    pub fn new(num_bits: usize, offset: usize, reset_value: u64) -> Self {
        Self {
            offset,
            num_bits,
            reset_value,
        }
    }

    pub fn last_bit(&self) -> usize {
        self.offset + self.num_bits - 1
    }

    /// Update as if written by instructions.
    pub fn apply_write_permissions(&self, value: u64, permissions: &Permission) -> u64 {
        match permissions {
            Permission::ReadOnly
            | Permission::ReadVolatileOnly
            | Permission::Reserved
            | Permission::WriteOneCommits
            | Permission::WriteIgnore => self.clear(value),
            _ => value,
        }
    }

    /// Update as if modified by the hardware.
    pub fn apply_set_permissions(&self, value: u64, permissions: &Permission) -> u64 {
        match permissions {
            Permission::ReadOnly
            | Permission::Reserved
            | Permission::WriteOneCommits
            | Permission::WriteIgnore => {
                let mask: u64 = (1 << self.num_bits) - 1;
                value & !((mask) << self.offset) | (self.reset_value << self.offset)
            }
            _ => value,
        }
    }

    /// Update full register value as if this field were being read by
    /// instructions
    pub fn apply_read_permissions(&self, value: u64, permissions: &Permission) -> u64 {
        let mut value = value;
        match permissions {
            Permission::WriteOnly => {
                value = self.clear(value);
            }
            _ => {
                // Leave value alone
            }
        }
        value
    }

    /// Access used by hardware.
    pub fn apply_reset_value(&self, value: u64) -> u64 {
        let mask: u64 = (1 << self.num_bits) - 1;
        value & !((mask) << self.offset) | (self.reset_value << self.offset)
    }

    pub fn clear(&self, value: u64) -> u64 {
        let mask: u64 = (1 << self.num_bits) - 1;
        value & !((mask) << self.offset)
    }

    pub fn value(&self, value: u64) -> u64 {
        let mask: u64 = (1 << self.num_bits) - 1;
        (value >> self.offset) & mask
    }
}
