// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! A virtual address space for a Processing Element (PE).
//!
//! Regions map aligned virtual addresses to destination devices and absolute
//! physical addresses.

use std::collections::BTreeMap;
use std::option::Option;
use std::vec::Vec;

use gwr_engine::sim_error;
use gwr_engine::types::{DeviceId, SimError};

use crate::memory::checked_last_address;

/// How accesses to a virtual region select their device and physical address.
pub enum AddressRegionPolicy {
    /// Send accesses to `device` without changing their addresses.
    Passthrough { device: DeviceId },
    /// Distribute successive access-sized chunks across banks in list order.
    ///
    /// The first chunk of the virtual region uses the first bank. After each
    /// bank receives one chunk, the offset within each bank advances by the
    /// access width.
    Striped {
        /// The capacity in bytes available in each bank.
        device_size_bytes: u64,
        /// Ordered pairs of destination device and absolute physical base.
        banks: Vec<(DeviceId, u64)>,
    },
}

/// A virtual region with inclusive start and end addresses.
struct AddressRegion {
    pub start: u64,
    pub end: u64,
    pub policy: AddressRegionPolicy,
}

/// Nonoverlapping virtual regions sharing one fixed, nonzero access width.
pub struct AddressSpace {
    access_size_bytes: u64,
    regions: BTreeMap<u64, AddressRegion>,
}

impl AddressSpace {
    /// Create an empty address space with the given access width in bytes.
    ///
    /// Returns an error if `access_size_bytes` is zero.
    pub fn new(access_size_bytes: u64) -> Result<Self, SimError> {
        if access_size_bytes == 0 {
            return sim_error!("Invalid access size {access_size_bytes}");
        }

        Ok(Self {
            access_size_bytes,
            regions: BTreeMap::new(),
        })
    }

    /// The fixed access width and stripe size, in bytes.
    #[must_use]
    pub fn access_size_bytes(&self) -> u64 {
        self.access_size_bytes
    }

    /// Map the inclusive virtual interval `[start, start + size - 1]`.
    ///
    /// Returns an error if the size is zero, the start or size is unaligned to
    /// the access width, the region overlaps an existing region, or its range
    /// overflows `u64`.
    ///
    /// Striped regions also require nonempty banks with aligned bases and bank
    /// capacity. Their combined capacity must equal `size`, and every physical
    /// bank range must fit in `u64`.
    pub fn insert(
        &mut self,
        start: u64,
        size: u64,
        policy: AddressRegionPolicy,
    ) -> Result<(), SimError> {
        if size == 0 {
            return sim_error!("Invalid region size {size}");
        }

        let access_size_bytes = self.access_size_bytes;
        if !start.is_multiple_of(access_size_bytes) {
            return sim_error!(
                "Region start {start} not aliged to access size {access_size_bytes}"
            );
        }
        if !size.is_multiple_of(access_size_bytes) {
            return sim_error!(
                "Region size {size} not a multiple of access size {access_size_bytes}"
            );
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

        match &policy {
            AddressRegionPolicy::Passthrough { .. } => (),

            AddressRegionPolicy::Striped {
                device_size_bytes,
                banks,
            } => {
                if !device_size_bytes.is_multiple_of(access_size_bytes) {
                    return sim_error!(
                        "Striped region bank size {device_size_bytes} not multiple of access size {access_size_bytes}"
                    );
                }

                for (_, addr) in banks {
                    if !addr.is_multiple_of(access_size_bytes) {
                        return sim_error!(
                            "Striped region bank address {addr} not aligned to access size {access_size_bytes}"
                        );
                    }
                }

                let bank_count = u64::try_from(banks.len()).map_err(|error| {
                    SimError(format!("bank count cannot be represented as u64: {error}"))
                })?;
                let spanned_bytes = bank_count
                    .checked_mul(*device_size_bytes)
                    .ok_or_else(|| SimError("Striped region capacity overflows u64".to_owned()))?;
                if spanned_bytes != size {
                    return sim_error!("Striped region banks span {spanned_bytes} of {size} bytes");
                }

                for (_, addr) in banks {
                    if checked_last_address(*addr, *device_size_bytes).is_none() {
                        return sim_error!(
                            "Striped region bank at {addr} with size {device_size_bytes} overflows the address space"
                        );
                    }
                }
            }
        }

        let region = AddressRegion { start, end, policy };
        self.regions.insert(start, region);

        Ok(())
    }

    /// Resolve an aligned virtual address to its destination device and
    /// address.
    ///
    /// Returns `Some((device, physical_addr))`, where `physical_addr` is an
    /// absolute physical address. Returns `None` if the virtual address is
    /// unaligned to the access width or is outside all mapped regions.
    #[must_use]
    pub fn resolve(&self, virtual_addr: u64) -> Option<(DeviceId, u64)> {
        if !virtual_addr.is_multiple_of(self.access_size_bytes) {
            return None;
        }

        // Find region with greatest start <= addr
        let (_, region) = self.regions.range(..=virtual_addr).next_back()?;
        if virtual_addr > region.end {
            return None;
        }

        let offset_in_region = virtual_addr - region.start;

        match &region.policy {
            AddressRegionPolicy::Passthrough { device } => Some((*device, virtual_addr)),
            AddressRegionPolicy::Striped {
                banks,
                device_size_bytes: _,
            } => {
                let stripe_index = offset_in_region / self.access_size_bytes;
                let bank_index = stripe_index as usize % banks.len();
                let offset_in_bank = (stripe_index / banks.len() as u64) * self.access_size_bytes;

                let (device, physical_base) = banks[bank_index];
                Some((device, physical_base + offset_in_bank))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gwr_engine::types::DeviceId;

    use crate::processing_element::address_space::{AddressRegionPolicy, AddressSpace};

    type PolicyFactory = fn(DeviceId, u64) -> AddressRegionPolicy;

    const POLICIES: [(&str, PolicyFactory); 2] = [
        ("passthrough", |device, _| {
            AddressRegionPolicy::Passthrough { device }
        }),
        ("striped", |device, size| AddressRegionPolicy::Striped {
            device_size_bytes: size / 2,
            banks: vec![(device, 0x0000_8000), (DeviceId(device.0 + 4), 0x0000_A000)],
        }),
    ];

    #[test]
    fn new_zero_access_size() {
        assert!(AddressSpace::new(0).is_err());
    }

    #[test]
    fn insert_successfully() {
        let address_space = setup_space();
        assert_eq!(address_space.regions.len(), 3);
    }

    #[test]
    fn insert_in_between() {
        for (name, policy) in POLICIES {
            let mut address_space = setup_space();
            address_space
                .insert(0x0000_3000, 0x1000, policy(DeviceId(4), 0x1000))
                .unwrap();
            assert_eq!(address_space.regions.len(), 4, "{name}");
        }
    }

    #[test]
    fn insert_zero_sized() {
        for (name, policy) in POLICIES {
            let mut address_space = setup_space();
            let error = address_space
                .insert(0x0000_6000, 0, policy(DeviceId(4), 0))
                .expect_err(name);
            assert!(
                error.to_string().contains("Invalid region size 0"),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn insert_unaligned_start() {
        for (name, policy) in POLICIES {
            let mut address_space = setup_space();
            let start = 0x0000_6001;
            assert_ne!(start % address_space.access_size_bytes(), 0);
            let error = address_space
                .insert(start, 0x1000, policy(DeviceId(4), 0x1000))
                .expect_err(name);
            assert!(
                error.to_string().contains("Region start"),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn insert_unaligned_size() {
        for (name, policy) in POLICIES {
            let mut address_space = setup_space();
            let size = 0x1001;
            assert_ne!(size % address_space.access_size_bytes(), 0);
            let error = address_space
                .insert(0x0000_6000, size, policy(DeviceId(4), size))
                .expect_err(name);
            assert!(error.to_string().contains("Region size"), "{name}: {error}");
        }
    }

    #[test]
    fn insert_ending_at_last_address() {
        for (name, policy) in POLICIES {
            let mut address_space = AddressSpace::new(32).unwrap();
            let start = u64::MAX - 63;
            address_space
                .insert(start, 64, policy(DeviceId(4), 64))
                .unwrap();
            assert_eq!(address_space.regions[&start].end, u64::MAX, "{name}");
            assert!(address_space.resolve(u64::MAX - 31).is_some(), "{name}");
        }
    }

    #[test]
    fn insert_range_overflow() {
        for (name, policy) in POLICIES {
            let mut address_space = AddressSpace::new(32).unwrap();
            let error = address_space
                .insert(u64::MAX - 63, 128, policy(DeviceId(4), 128))
                .expect_err(name);
            assert!(
                error.to_string().contains("overflows the address space"),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn insert_overlap() {
        for (name, policy) in POLICIES {
            let mut address_space = setup_space();
            let error = address_space
                .insert(0x0000_0F00, 0x200, policy(DeviceId(3), 0x200))
                .expect_err(name);
            assert!(
                error.to_string().contains("Region overlap"),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn insert_overlap_inserted() {
        for (name, policy) in POLICIES {
            let mut address_space = setup_space();
            let error = address_space
                .insert(0x0000_3000, 0x2000, policy(DeviceId(3), 0x2000))
                .expect_err(name);
            assert!(
                error.to_string().contains("Region overlap"),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn insert_striped_unaligned_bank_size() {
        let mut address_space = setup_space();
        let device_size_bytes = 0x810;
        assert_ne!(device_size_bytes % address_space.access_size_bytes(), 0);
        let error = address_space
            .insert(
                0x0000_6000,
                0x1020,
                AddressRegionPolicy::Striped {
                    device_size_bytes,
                    banks: vec![(DeviceId(4), 0x0000_8000), (DeviceId(8), 0x0000_A000)],
                },
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("Striped region bank size"),
            "{error}"
        );
    }

    #[test]
    fn insert_striped_unaligned_bank_address() {
        for misaligned_bank in [0, 1] {
            let mut address_space = setup_space();
            let mut banks = vec![(DeviceId(4), 0x0000_8000), (DeviceId(8), 0x0000_A000)];
            banks[misaligned_bank].1 += 1;
            let error = address_space
                .insert(
                    0x0000_6000,
                    0x1000,
                    AddressRegionPolicy::Striped {
                        device_size_bytes: 0x800,
                        banks,
                    },
                )
                .unwrap_err();
            assert!(
                error.to_string().contains("Striped region bank address"),
                "bank {misaligned_bank}: {error}"
            );
        }
    }

    #[test]
    fn insert_striped_mismatched_bank_span() {
        for size in [0x800, 0x2000] {
            let mut address_space = setup_space();
            let device_size_bytes = 0x800;
            assert_ne!(device_size_bytes * 2, size);
            let error = address_space
                .insert(
                    0x0000_6000,
                    size,
                    AddressRegionPolicy::Striped {
                        device_size_bytes,
                        banks: vec![(DeviceId(4), 0x0000_8000), (DeviceId(8), 0x0000_A000)],
                    },
                )
                .unwrap_err();
            assert!(
                error.to_string().contains("Striped region banks span"),
                "size {size}: {error}"
            );
        }
    }

    #[test]
    fn insert_striped_capacity_overflow() {
        let mut address_space = AddressSpace::new(32).unwrap();
        let device_size_bytes = 1_u64 << 63;
        assert!(device_size_bytes.checked_mul(3).is_none());
        let error = address_space
            .insert(
                0,
                device_size_bytes,
                AddressRegionPolicy::Striped {
                    device_size_bytes,
                    banks: vec![
                        (DeviceId(4), 0x0000_8000),
                        (DeviceId(8), 0x0000_A000),
                        (DeviceId(12), 0x0000_C000),
                    ],
                },
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("capacity overflows u64"),
            "{error}"
        );
    }

    #[test]
    fn insert_striped_bank_ending_at_last_address() {
        let mut address_space = AddressSpace::new(32).unwrap();
        let base = u64::MAX - 63;
        address_space
            .insert(
                0,
                64,
                AddressRegionPolicy::Striped {
                    device_size_bytes: 64,
                    banks: vec![(DeviceId(4), base)],
                },
            )
            .unwrap();
        assert_eq!(address_space.resolve(0), Some((DeviceId(4), base)));
        assert_eq!(
            address_space.resolve(32),
            Some((DeviceId(4), u64::MAX - 31))
        );
    }

    #[test]
    fn insert_striped_bank_range_overflow() {
        for overflowing_bank in [0, 1] {
            let mut address_space = AddressSpace::new(32).unwrap();
            let mut banks = vec![(DeviceId(4), 0x0000_8000), (DeviceId(8), 0x0000_A000)];
            banks[overflowing_bank].1 = u64::MAX - 31;
            let error = address_space
                .insert(
                    0,
                    128,
                    AddressRegionPolicy::Striped {
                        device_size_bytes: 64,
                        banks,
                    },
                )
                .unwrap_err();
            assert!(
                error.to_string().contains("Striped region bank at"),
                "bank {overflowing_bank}: {error}"
            );
        }
    }

    #[test]
    fn resolve_unaligned_address() {
        let address_space = setup_space();
        for aligned_address in [0x0000_0020, 0x0000_2020, 0x0000_4020] {
            assert!(address_space.resolve(aligned_address).is_some());
            for offset in [1, 31] {
                let address = aligned_address + offset;
                assert_eq!(address_space.resolve(address), None, "{address:#x}");
            }
        }
    }

    #[test]
    fn resolve_unmapped_address() {
        assert_eq!(AddressSpace::new(32).unwrap().resolve(0), None);

        let address_space = setup_space();
        for address in [
            0x0000_1000,
            0x0000_1FE0,
            0x0000_3000,
            0x0000_3FE0,
            0x0000_5000,
            0x0000_8000,
            0x0000_A000,
            u64::MAX - 31,
        ] {
            assert_eq!(address_space.resolve(address), None, "{address:#x}");
        }
    }

    #[test]
    fn resolve_passthrough_address() {
        let address_space = setup_space();
        for (address, device) in [
            (0x0000_0000, DeviceId(1)),
            (0x0000_0020, DeviceId(1)),
            (0x0000_0FE0, DeviceId(1)),
            (0x0000_2000, DeviceId(2)),
            (0x0000_2020, DeviceId(2)),
            (0x0000_2FE0, DeviceId(2)),
        ] {
            assert_eq!(
                address_space.resolve(address),
                Some((device, address)),
                "{address:#x}"
            );
        }
    }

    #[test]
    fn resolve_striped_address() {
        let address_space = setup_space();
        for (address, device, physical_address) in [
            (0x0000_4000, DeviceId(3), 0x0000_8000),
            (0x0000_4020, DeviceId(7), 0x0000_A000),
            (0x0000_4040, DeviceId(3), 0x0000_8020),
            (0x0000_4060, DeviceId(7), 0x0000_A020),
            (0x0000_4FC0, DeviceId(3), 0x0000_87E0),
            (0x0000_4FE0, DeviceId(7), 0x0000_A7E0),
        ] {
            assert_eq!(
                address_space.resolve(address),
                Some((device, physical_address)),
                "{address:#x}"
            );
        }
    }

    fn setup_space() -> AddressSpace {
        let mut address_space = AddressSpace::new(32).unwrap();
        address_space
            .insert(
                0x0000_0000,
                0x1000,
                AddressRegionPolicy::Passthrough {
                    device: DeviceId(1),
                },
            )
            .unwrap();
        address_space
            .insert(
                0x0000_2000,
                0x1000,
                AddressRegionPolicy::Passthrough {
                    device: DeviceId(2),
                },
            )
            .unwrap();
        address_space
            .insert(
                0x0000_4000,
                0x1000,
                AddressRegionPolicy::Striped {
                    device_size_bytes: 0x800,
                    banks: vec![(DeviceId(3), 0x0000_8000), (DeviceId(7), 0x0000_A000)],
                },
            )
            .unwrap();
        address_space
    }
}
