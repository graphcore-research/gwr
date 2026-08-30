// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::*;

#[test]
fn accepts_memory_ending_at_the_final_physical_byte() {
    let mut config = platform();
    config.memories = Some(vec![memory("top", u64::MAX, 1)]);

    config.validate().unwrap();
}

#[test]
fn rejects_a_memory_range_past_the_final_physical_byte() {
    let mut config = platform();
    config.memories = Some(vec![memory("overflowing", u64::MAX, 2)]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains(
        "Memory 'overflowing': address range starting at 0xffffffffffffffff with capacity 2 bytes exceeds the physical address space"
    ));
}

#[test]
fn accepts_adjacent_memories_at_the_end_of_the_address_space() {
    let mut config = platform();
    config.memories = Some(vec![
        memory("penultimate", u64::MAX - 1, 1),
        memory("final", u64::MAX, 1),
    ]);

    config.validate().unwrap();
}

#[test]
fn rejects_a_referenced_zero_capacity_memory() {
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &["hbm0"])];
    config.memories = Some(vec![memory("hbm0", 0, 0)]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Memory 'hbm0': capacity must be greater than zero"));
}

#[test]
fn rejects_an_unreferenced_zero_capacity_memory() {
    let mut config = platform();
    config.memories = Some(vec![memory("unused", 0, 0)]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Memory 'unused': capacity must be greater than zero"));
}

#[test]
fn rejects_overlapping_physical_memories() {
    let mut config = platform();
    config.memories = Some(vec![memory("hbm0", 0, 1024), memory("hbm1", 512, 1024)]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Physical memory ranges overlap"));
    assert!(error.contains("'hbm0' (0x0..=0x3ff)"));
    assert!(error.contains("'hbm1' (0x200..=0x5ff)"));
}
