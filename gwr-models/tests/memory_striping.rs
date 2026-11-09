// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::collections::HashSet;

use gwr_engine::types::DeviceId;
use gwr_models::memory::memory_map::{MemoryMap, MemoryRegion};

fn members(n: u64) -> Vec<MemoryRegion> {
    (0..n)
        .map(|i| MemoryRegion {
            start: 0x1000 + i * 256,
            end: 0x10ff + i * 256,
            device: DeviceId(i),
        })
        .collect()
}

#[test]
fn striped_resolution_is_a_bijection() {
    for n in [2, 3, 4] {
        let mut map = MemoryMap::new();
        map.insert_striped(members(n), 64).unwrap();
        let mut seen = HashSet::new();
        for a in 0..n * 256 {
            let r = map.resolve(0x1000 + a).unwrap();
            assert_eq!(r.device, DeviceId(a / 64 % n));
            assert_eq!(
                r.physical_address,
                0x1000 + r.device.0 * 256 + r.offset_in_device
            );
            assert_eq!(
                (r.offset_in_device / 64 * n + r.device.0) * 64 + r.offset_in_device % 64,
                a
            );
            assert_eq!(r.contiguous_bytes, u128::from(64 - a % 64));
            assert!(seen.insert((r.device.0, r.offset_in_device)));
        }
        assert!(map.resolve(0x1000 + n * 256).is_none());
        assert!(map.resolve_access(0x103f, 2).is_err());
        assert!(map.resolve_access(0x1000, 0).is_err());
        assert_eq!(
            map.logical_ranges().collect::<Vec<_>>(),
            vec![0x1000..u128::from(0x1000 + n * 256)]
        );
        assert!(!map.unmap(0x1040));
        assert!(map.unmap(0x1000));
        map.insert_striped(members(n), 64).unwrap();
    }
}

#[test]
fn group_order_and_contiguous_parity() {
    let mut map = MemoryMap::new();
    let mut devices = members(2);
    devices.reverse();
    map.insert_striped(devices, 64).unwrap();
    assert_eq!(map.resolve(0x1000).unwrap().physical_address, 0x1100);
    map.insert(0x2000, 128, DeviceId(9)).unwrap();
    let r = map.resolve(0x2040).unwrap();
    assert_eq!(r.physical_address, 0x2040);
    assert_eq!(map.lookup(0x2040), Some((DeviceId(9), 64)));
    assert!(!map.is_striped_address(0x2040));
    assert!(map.is_striped_address(0x1000));
}

#[test]
fn rejects_invalid_groups_without_mutation() {
    for stripe in [0, 3, 512] {
        let mut map = MemoryMap::new();
        assert!(map.insert_striped(members(2), stripe).is_err());
        assert_eq!(map.num_regions(), 0);
    }
    for devices in [
        vec![],
        members(1),
        vec![members(2)[0].clone(); 2],
        vec![members(3)[0].clone(), members(3)[2].clone()],
    ] {
        assert!(MemoryMap::new().insert_striped(devices, 64).is_err());
    }
    let mut unequal = members(2);
    unequal[1].end += 64;
    assert!(MemoryMap::new().insert_striped(unequal, 64).is_err());
}

#[test]
fn last_u64_byte_is_valid() {
    let mut map = MemoryMap::new();
    map.insert_striped(
        vec![
            MemoryRegion {
                start: u64::MAX - 127,
                end: u64::MAX - 64,
                device: DeviceId(0),
            },
            MemoryRegion {
                start: u64::MAX - 63,
                end: u64::MAX,
                device: DeviceId(1),
            },
        ],
        64,
    )
    .unwrap();
    let r = map.resolve_access(u64::MAX, 1).unwrap();
    assert_eq!(r.physical_address, u64::MAX);
    assert!(map.resolve_access(u64::MAX, 2).is_err());
}

#[test]
fn generators_translate_and_reject_cross_stripe_geometry() {
    use std::rc::Rc;

    use gwr_engine::engine::Engine;
    use gwr_engine::traits::Routable;
    use gwr_models::memory::memory_access_gen::random::Random;
    use gwr_models::memory::memory_access_gen::strided::Strided;
    let engine = Engine::default();
    let mut map = MemoryMap::new();
    map.insert_striped(members(2), 64).unwrap();
    let map = Rc::new(map);
    let stream = Strided::new(
        engine.top(),
        "s",
        &map,
        0x1000,
        0x1000,
        0x1200,
        64,
        0,
        64,
        8,
    )
    .unwrap();
    let addresses = stream.map(|request| request.dst_addr()).collect::<Vec<_>>();
    assert_eq!(
        addresses,
        [
            0x1000, 0x1100, 0x1040, 0x1140, 0x1080, 0x1180, 0x10c0, 0x11c0
        ]
    );
    assert!(
        Strided::new(
            engine.top(),
            "bad",
            &map,
            0x1000,
            0x1030,
            0x1200,
            64,
            0,
            64,
            8
        )
        .is_err()
    );
    assert!(
        Random::new(
            engine.top(),
            "bad",
            42,
            &map,
            0x1000,
            0x1000,
            0x1200,
            u64::MAX,
            0,
            64,
            8
        )
        .is_err()
    );
    let random = Random::new(
        engine.top(),
        "r",
        42,
        &map,
        0x1000,
        0x1000,
        0x1200,
        !63,
        0,
        64,
        100,
    )
    .unwrap();
    assert_eq!(random.count(), 100);
}
