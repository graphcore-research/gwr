// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::{DeviceId, SimError};

use crate::memory::checked_last_address;

#[derive(Clone)]
pub struct MemoryMap {
    // key = start address of region
    regions: BTreeMap<u64, MemoryRegion>,
}

impl Default for MemoryMap {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryMap {
    #[must_use]
    pub fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
        }
    }

    /// Map a [start, start+size-1] region to a device.
    pub fn insert(&mut self, start: u64, size: u64, device: DeviceId) -> Result<(), SimError> {
        if size == 0 {
            return sim_error!("Invalid region size {size}");
        }
        let end = checked_last_address(start, size).ok_or_else(|| {
            SimError(format!(
                "Region at {start} with size {size} overflows the address space"
            ))
        })?;

        // Check overlap with previous region (if any)
        if let Some((_, prev)) = self.regions.range(..=start).next_back()
            && prev.end >= start
        {
            return sim_error!("Region overlap at {start}");
        }

        // Check overlap with next region (if any)
        if let Some((_, next)) = self.regions.range(start..).next()
            && next.start <= end
        {
            return sim_error!("Region overlap at {end}");
        }

        let region = MemoryRegion { start, end, device };
        self.regions.insert(start, region);
        Ok(())
    }

    /// Remove a region by its exact start address.
    #[must_use]
    pub fn unmap(&mut self, start: u64) -> Option<MemoryRegion> {
        self.regions.remove(&start)
    }

    /// Resolve an address to (device_id, offset_in_region).
    #[must_use]
    pub fn lookup(&self, addr: u64) -> Option<(DeviceId, u64)> {
        let region = self.region_at(addr)?;
        Some((region.device, addr - region.start))
    }

    /// Return the device if a nonempty request fits entirely within one memory
    /// region.
    pub(crate) fn resolve_access(&self, address: u64, bytes: usize) -> Result<DeviceId, SimError> {
        let region = self
            .region_at(address)
            .ok_or_else(|| SimError(format!("Unmapped address {address:#x}")))?;
        let contiguous_bytes = u128::from(region.end) + 1 - u128::from(address);
        if bytes == 0 || bytes as u128 > contiguous_bytes {
            return sim_error!(
                "Access at {address:#x} of {bytes} bytes crosses a map boundary or is empty"
            );
        }
        Ok(region.device)
    }

    fn region_at(&self, addr: u64) -> Option<&MemoryRegion> {
        let (_, region) = self.regions.range(..=addr).next_back()?;
        (addr <= region.end).then_some(region)
    }

    /// Each mapped interval exactly once.
    pub(crate) fn logical_ranges(&self) -> impl Iterator<Item = Range<u128>> + '_ {
        self.regions
            .values()
            .map(|r| u128::from(r.start)..u128::from(r.end) + 1)
    }

    #[must_use]
    pub fn num_regions(&self) -> usize {
        self.regions.len()
    }

    /// Iterate all mapped ranges.
    pub fn regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions.values()
    }
}

#[derive(Clone, Debug)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
    pub device: DeviceId,
}

#[cfg(test)]
mod tests {
    use gwr_engine::types::DeviceId;

    use crate::memory::memory_map::MemoryMap;

    fn setup_map() -> MemoryMap {
        let mut memory_map = MemoryMap::new();
        memory_map.insert(0x0000_0000, 0x1000, DeviceId(1)).unwrap();
        memory_map.insert(0x0000_2000, 0x1000, DeviceId(2)).unwrap();
        memory_map.insert(0x0000_4000, 0x1000, DeviceId(3)).unwrap();
        memory_map
    }

    #[test]
    fn inserting_disjoint_regions_increases_region_count() {
        let memory_map = setup_map();
        assert_eq!(memory_map.num_regions(), 3);
    }

    #[test]
    fn inserts_a_region_in_a_gap_between_existing_regions() {
        let mut memory_map = setup_map();
        memory_map.insert(0x0000_3000, 0x1000, DeviceId(4)).unwrap();
        assert_eq!(memory_map.num_regions(), 4);
    }

    #[test]
    #[should_panic(expected = "Region overlap")]
    fn rejects_a_region_starting_inside_an_existing_region() {
        let mut memory_map = setup_map();
        memory_map.insert(0x0000_0F00, 0x200, DeviceId(3)).unwrap();
    }

    #[test]
    #[should_panic(expected = "Region overlap")]
    fn rejects_a_region_overlapping_a_later_existing_region() {
        let mut memory_map = setup_map();
        memory_map.insert(0x0000_3000, 0x2000, DeviceId(3)).unwrap();
    }

    #[test]
    fn lookup_returns_device_and_relative_offset() {
        let memmory_map = setup_map();
        let (dev, offset) = memmory_map.lookup(0x0000_2004).unwrap();

        assert_eq!(dev, DeviceId(2));
        assert_eq!(offset, 0x4);
    }

    #[test]
    fn lookup_includes_the_first_byte_of_a_region() {
        let memmory_map = setup_map();
        let (dev, offset) = memmory_map.lookup(0x0000_4000).unwrap();

        assert_eq!(dev, DeviceId(3));
        assert_eq!(offset, 0x0);
    }

    #[test]
    fn lookup_includes_the_last_byte_of_a_region() {
        let memmory_map = setup_map();
        let (dev, offset) = memmory_map.lookup(0x0000_4fff).unwrap();

        assert_eq!(dev, DeviceId(3));
        assert_eq!(offset, 0xfff);
    }

    #[test]
    fn lookup_excludes_the_address_immediately_after_a_region() {
        let memmory_map = setup_map();
        assert!(memmory_map.lookup(0x0000_5000).is_none());
    }

    #[test]
    fn adjacent_regions_can_end_at_last_address() {
        let mut memory_map = MemoryMap::new();
        memory_map.insert(u64::MAX - 1, 1, DeviceId(1)).unwrap();
        memory_map.insert(u64::MAX, 1, DeviceId(2)).unwrap();

        assert_eq!(memory_map.num_regions(), 2);
        assert_eq!(memory_map.lookup(u64::MAX), Some((DeviceId(2), 0)));
    }

    #[test]
    fn insert_rejects_range_overflow() {
        let mut memory_map = MemoryMap::new();

        assert!(memory_map.insert(u64::MAX, 2, DeviceId(1)).is_err());
    }

    #[test]
    #[should_panic(expected = "Invalid region size 0")]
    fn rejects_zero_sized_regions() {
        let mut memory_map = setup_map();
        memory_map.insert(0x0000_8000, 0x0, DeviceId(4)).unwrap();
    }
}
