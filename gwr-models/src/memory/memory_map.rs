// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::{DeviceId, SimError};

use crate::memory::checked_last_address;
mod striping;
use striping::Striping;

/// One physical memory's inclusive address window.
#[derive(Clone, Debug)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
    pub device: DeviceId,
}

/// A logical address resolved into a unique physical memory location.
#[derive(Clone, Debug)]
pub struct ResolvedAddress {
    pub device: DeviceId,
    pub physical_address: u64,
    pub offset_in_device: u64,
    /// Consecutive bytes remaining before a destination or mapping boundary.
    pub contiguous_bytes: u128,
}

#[derive(Clone)]
struct Window {
    region: MemoryRegion,
    striping: Option<Striping>,
}

/// Compact address map with optional round-robin physical-memory striping.
#[derive(Clone, Default)]
pub struct MemoryMap {
    windows: BTreeMap<u64, Window>,
}

impl MemoryMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Map a nonempty contiguous physical window.
    pub fn insert(&mut self, start: u64, size: u64, device: DeviceId) -> Result<(), SimError> {
        if size == 0 {
            return sim_error!("Invalid region size {size}");
        }
        let end = checked_last_address(start, size)
            .ok_or_else(|| SimError("Region overflows address space".into()))?;
        self.insert_window(Window {
            region: MemoryRegion { start, end, device },
            striping: None,
        })
    }

    /// Map the existing contiguous capacity of equally sized physical memories
    /// using explicit member order and round-robin stripes. No aliases are
    /// added.
    pub fn insert_striped(
        &mut self,
        members: Vec<MemoryRegion>,
        stripe_bytes: u64,
    ) -> Result<(), SimError> {
        let striping = Striping::new(members, stripe_bytes)?;
        let region = striping.logical_region();
        self.insert_window(Window {
            region,
            striping: Some(striping),
        })
    }

    fn insert_window(&mut self, window: Window) -> Result<(), SimError> {
        let MemoryRegion { start, end, .. } = window.region;
        if self
            .windows
            .range(..=start)
            .next_back()
            .is_some_and(|(_, prev)| prev.region.end >= start)
            || self
                .windows
                .range(start..)
                .next()
                .is_some_and(|(_, next)| next.region.start <= end)
        {
            return sim_error!("Region overlap at {start:#x}");
        }
        self.windows.insert(start, window);
        Ok(())
    }

    /// Remove an entire logical window at its exact base, reporting existence.
    #[must_use]
    pub fn unmap(&mut self, start: u64) -> bool {
        self.windows.remove(&start).is_some()
    }

    /// Resolve to (physical device, offset within that device).
    #[must_use]
    pub fn lookup(&self, addr: u64) -> Option<(DeviceId, u64)> {
        self.resolve(addr).map(|r| (r.device, r.offset_in_device))
    }

    /// Resolve one logical byte. Wire requests must use `physical_address`.
    #[must_use]
    pub fn resolve(&self, addr: u64) -> Option<ResolvedAddress> {
        let window = self.window_at(addr)?;
        if let Some(striping) = &window.striping {
            return Some(striping.resolve(addr));
        }
        Some(ResolvedAddress {
            device: window.region.device,
            physical_address: addr,
            offset_in_device: addr - window.region.start,
            contiguous_bytes: u128::from(window.region.end) + 1 - u128::from(addr),
        })
    }

    /// Resolve a nonempty request wholly contained in one consecutive physical
    /// range.
    pub fn resolve_access(&self, address: u64, bytes: usize) -> Result<ResolvedAddress, SimError> {
        let resolved = self
            .resolve(address)
            .ok_or_else(|| SimError(format!("Unmapped address {address:#x}")))?;
        if bytes == 0 || bytes as u128 > resolved.contiguous_bytes {
            return sim_error!(
                "Access at {address:#x} of {bytes} bytes crosses a stripe/map boundary or is empty"
            );
        }
        Ok(resolved)
    }

    /// Whether this logical address uses an active striped window.
    #[must_use]
    pub fn is_striped_address(&self, address: u64) -> bool {
        self.window_at(address)
            .is_some_and(|w| w.striping.is_some())
    }

    /// Check that ordinary aligned fetches cannot straddle a stripe.
    pub fn validate_fetch_size(&self, address: u64, bytes: usize) -> Result<(), SimError> {
        let window = self
            .window_at(address)
            .ok_or_else(|| SimError("Unmapped fetch address".into()))?;
        if bytes == 0
            || window
                .striping
                .as_ref()
                .is_some_and(|s| !s.supports_fetch(bytes as u64))
        {
            return sim_error!("LSU fetch size {bytes} incompatible with stripe geometry");
        }
        Ok(())
    }

    fn window_at(&self, addr: u64) -> Option<&Window> {
        let (_, w) = self.windows.range(..=addr).next_back()?;
        (addr <= w.region.end).then_some(w)
    }

    /// Logical coverage containing this byte, independent of physical
    /// destinations.
    pub(crate) fn logical_range_at(&self, addr: u64) -> Option<Range<u128>> {
        self.window_at(addr)
            .map(|w| u128::from(w.region.start)..u128::from(w.region.end) + 1)
    }

    #[must_use]
    pub fn num_regions(&self) -> usize {
        self.windows.len()
    }

    /// Iterate each mapped logical window, without per-stripe expansion.
    pub fn regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.windows.values().map(|window| &window.region)
    }

    /// Each mapped logical interval exactly once, without per-stripe expansion.
    pub fn logical_ranges(&self) -> impl Iterator<Item = Range<u128>> + '_ {
        self.windows
            .values()
            .map(|w| u128::from(w.region.start)..u128::from(w.region.end) + 1)
    }
}

#[cfg(test)]
mod tests {
    use gwr_engine::types::DeviceId;

    use crate::memory::memory_map::MemoryMap;

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
    fn insert_zero_sized() {
        let mut memory_map = setup_map();
        memory_map.insert(0x0000_8000, 0x0, DeviceId(4)).unwrap();
    }

    fn setup_map() -> MemoryMap {
        let mut memory_map = MemoryMap::new();
        memory_map.insert(0x0000_0000, 0x1000, DeviceId(1)).unwrap();
        memory_map.insert(0x0000_2000, 0x1000, DeviceId(2)).unwrap();
        memory_map.insert(0x0000_4000, 0x1000, DeviceId(3)).unwrap();
        memory_map
    }
}
