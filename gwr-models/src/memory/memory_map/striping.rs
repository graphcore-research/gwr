// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::collections::HashSet;

use gwr_engine::types::SimError;

use super::{MemoryRegion, ResolvedAddress};

#[derive(Clone, Debug)]
pub(super) struct Striping {
    members: Vec<MemoryRegion>,
    stripe: u64,
    start: u64,
    end: u64,
}
impl Striping {
    pub(super) fn new(members: Vec<MemoryRegion>, stripe: u64) -> Result<Self, SimError> {
        let invalid = || {
            SimError("Invalid stripe group: require distinct equal-capacity adjacent memories, aligned to a nonzero power-of-two stripe".into())
        };
        if members.len() < 2 || !stripe.is_power_of_two() {
            return Err(invalid());
        }
        let mut ordered = members.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|r| r.start);
        let capacity = u128::from(ordered[0].end)
            .checked_sub(u128::from(ordered[0].start))
            .ok_or_else(invalid)?
            + 1;
        let mut ids = HashSet::new();
        for member in &ordered {
            if member.end < member.start
                || !ids.insert(member.device.0)
                || u128::from(member.end) + 1 - u128::from(member.start) != capacity
                || !member.start.is_multiple_of(stripe)
                || !capacity.is_multiple_of(u128::from(stripe))
            {
                return Err(invalid());
            }
        }
        if ordered
            .windows(2)
            .any(|p| u128::from(p[0].end) + 1 != u128::from(p[1].start))
        {
            return Err(invalid());
        }
        let start = ordered[0].start;
        let end = ordered.last().unwrap().end;
        Ok(Self {
            members,
            stripe,
            start,
            end,
        })
    }
    pub(super) fn members(&self) -> &[MemoryRegion] {
        &self.members
    }
    pub(super) fn stripe_bytes(&self) -> u64 {
        self.stripe
    }
    pub(super) fn logical_region(&self) -> MemoryRegion {
        MemoryRegion {
            start: self.start,
            end: self.end,
            device: self.members[0].device,
        }
    }
    pub(super) fn supports_fetch(&self, bytes: u64) -> bool {
        self.stripe.is_multiple_of(bytes)
    }
    pub(super) fn resolve(&self, address: u64) -> ResolvedAddress {
        let relative = address - self.start;
        let line = relative / self.stripe;
        let n = self.members.len() as u64;
        let member = &self.members[(line % n) as usize];
        let offset = line / n * self.stripe + relative % self.stripe;
        ResolvedAddress {
            device: member.device,
            physical_address: member.start + offset,
            offset_in_device: offset,
            contiguous_bytes: u128::from(self.stripe - relative % self.stripe),
        }
    }
}
