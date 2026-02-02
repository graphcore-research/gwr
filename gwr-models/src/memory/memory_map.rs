// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId(pub u64);

#[derive(Clone, Debug)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
    pub device: DeviceId,
}

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
        let end = if size > 0 {
            start + size - 1
        } else {
            return sim_error!("Invalid region size {size}");
        };

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
        // Find region with greatest start <= addr
        let (_, region) = self.regions.range(..=addr).next_back()?;
        if addr <= region.end {
            let offset = addr - region.start;
            Some((region.device, offset))
        } else {
            None
        }
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

#[cfg(test)]
mod tests {
    use crate::memory::memory_map::{DeviceId, MemoryMap};

    fn setup_map() -> MemoryMap {
        let mut memory_map = MemoryMap::new();
        memory_map.insert(0x0000_0000, 0x1000, DeviceId(1)).unwrap();
        memory_map.insert(0x0000_2000, 0x1000, DeviceId(2)).unwrap();
        memory_map.insert(0x0000_4000, 0x1000, DeviceId(3)).unwrap();
        memory_map
    }

    #[test]
    fn insert_successfully() {
        let memory_map = setup_map();
        assert_eq!(memory_map.num_regions(), 3);
    }

    #[test]
    fn insert_in_between() {
        let mut memory_map = setup_map();
        memory_map.insert(0x0000_3000, 0x1000, DeviceId(4)).unwrap();
        assert_eq!(memory_map.num_regions(), 4);
    }

    #[test]
    #[should_panic(expected = "Region overlap")]
    fn insert_overlap() {
        let mut memory_map = setup_map();
        memory_map.insert(0x0000_0F00, 0x200, DeviceId(3)).unwrap();
    }

    #[test]
    #[should_panic(expected = "Region overlap")]
    fn insert_overlap_inserted() {
        let mut memory_map = setup_map();
        memory_map.insert(0x0000_3000, 0x2000, DeviceId(3)).unwrap();
    }

    #[test]
    fn address_lookup() {
        let memmory_map = setup_map();
        let (dev, offset) = memmory_map.lookup(0x0000_2004).unwrap();

        assert_eq!(dev, DeviceId(2));
        assert_eq!(offset, 0x4);
    }

    #[test]
    fn address_lookup_begin() {
        let memmory_map = setup_map();
        let (dev, offset) = memmory_map.lookup(0x0000_4000).unwrap();

        assert_eq!(dev, DeviceId(3));
        assert_eq!(offset, 0x0);
    }

    #[test]
    fn address_lookup_end() {
        let memmory_map = setup_map();
        let (dev, offset) = memmory_map.lookup(0x0000_4fff).unwrap();

        assert_eq!(dev, DeviceId(3));
        assert_eq!(offset, 0xfff);
    }

    #[test]
    fn address_lookup_after() {
        let memmory_map = setup_map();
        assert!(memmory_map.lookup(0x0000_5000).is_none());
    }

    #[test]
    #[should_panic(expected = "Invalid region size 0")]
    fn insert_zero_sized() {
        let mut memory_map = setup_map();
        memory_map.insert(0x0000_8000, 0x0, DeviceId(4)).unwrap();
    }
}
