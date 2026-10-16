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

#[test]
fn rejects_lsu_mappings_with_unaligned_bases() {
    for (base, capacity, access_bytes) in [(1, 64, None), (25, 48, Some(24))] {
        let mut pe = processing_element("pe0", "mm0");
        pe.config.lsu_access_bytes = access_bytes;
        let mut config = platform();
        config.memory_maps = vec![memory_map("mm0", &["hbm0"])];
        config.memories = Some(vec![memory("hbm0", base, capacity)]);
        config.processing_elements = Some(vec![pe]);

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Processing element 'pe0'"), "{error}");
        assert!(error.contains("memory map 'mm0'"), "{error}");
        assert!(error.contains("Memory 'hbm0': base address"), "{error}");
        assert!(error.contains("LSU access size"), "{error}");
    }
}

#[test]
fn rejects_lsu_mappings_with_partial_final_blocks() {
    for (base, capacity, access_bytes) in [(0, 33, None), (48, 25, Some(24))] {
        let mut pe = processing_element("pe0", "mm0");
        pe.config.lsu_access_bytes = access_bytes;
        let mut config = platform();
        config.memory_maps = vec![memory_map("mm0", &["hbm0"])];
        config.memories = Some(vec![memory("hbm0", base, capacity)]);
        config.processing_elements = Some(vec![pe]);

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Memory 'hbm0': capacity"), "{error}");
        assert!(error.contains("LSU access size"), "{error}");
    }
}

#[test]
fn accepts_lsu_mappings_containing_whole_blocks() {
    for (base, capacity, access_bytes) in [
        (0, 64, 32),
        (24, 48, 24),
        (u64::MAX - 31, 32, 32),
        (u64::MAX, 1, 1),
    ] {
        let mut pe = processing_element("pe0", "mm0");
        pe.config.lsu_access_bytes = Some(access_bytes);
        let mut config = platform();
        config.memory_maps = vec![memory_map("mm0", &["hbm0"])];
        config.memories = Some(vec![memory("hbm0", base, capacity)]);
        config.processing_elements = Some(vec![pe]);

        config.validate().unwrap();
    }
}

#[test]
fn validates_lsu_alignment_for_every_pe_sharing_a_map() {
    let mut small_pe = processing_element("small", "mm0");
    small_pe.config.lsu_access_bytes = Some(16);
    let mut large_pe = processing_element("large", "mm0");
    large_pe.config.lsu_access_bytes = Some(32);
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &["hbm0"])];
    config.memories = Some(vec![memory("hbm0", 16, 48)]);
    config.processing_elements = Some(vec![small_pe, large_pe]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Processing element 'large'"), "{error}");
    assert!(error.contains("LSU access size 32"), "{error}");
}

#[test]
fn rejects_unaligned_memories_outside_the_pe_map() {
    let mut config = platform();
    config.memory_maps = vec![
        memory_map("mm0", &["hbm0"]),
        memory_map("unused", &["other"]),
    ];
    config.memories = Some(vec![memory("hbm0", 0, 64), memory("other", 65, 1)]);
    config.processing_elements = Some(vec![processing_element("pe0", "mm0")]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Memory 'other'"), "{error}");
    assert!(error.contains("32-byte fetch size"), "{error}");
}

#[test]
fn rejects_memory_boundaries_not_aligned_to_cache_lines() {
    for (base, bytes) in [(32, 256), (0, 160)] {
        let mut config = platform();
        let mut l2 = cache("l2");
        l2.config.line_size_bytes = Some(128);
        config.caches = Some(vec![l2]);
        config.memories = Some(vec![memory("hbm0", base, bytes)]);
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("hbm0"));
        assert!(error.contains("128"));
        assert!(error.contains("l2"));
    }
}

#[test]
fn checks_all_memories_against_effective_lsu_and_cache_sizes() {
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &["used"])];
    config.processing_elements = Some(vec![processing_element("pe0", "mm0")]);
    config.memories = Some(vec![memory("used", 0, 256), memory("unused", 257, 32)]);
    assert!(config.validate().is_err());
    config.memories.as_mut().unwrap()[1].base_address = 256;
    config.validate().unwrap();
    config.processing_elements.as_mut().unwrap()[0]
        .config
        .lsu_access_bytes = Some(64);
    assert!(config.validate().is_err());
    config.memories.as_mut().unwrap()[1].config.capacity_bytes = 192;
    let mut l2 = cache("l2");
    l2.config.line_size_bytes = Some(48);
    config.caches = Some(vec![l2]);
    // 256 is aligned to the largest size (64), but not to 48.
    assert!(config.validate().is_err());
    config.memories.as_mut().unwrap()[0].config.capacity_bytes = 192;
    config.memories.as_mut().unwrap()[1].base_address = 192;
    config.validate().unwrap();
}

#[test]
fn accepts_aligned_cache_memory_ending_at_the_final_physical_byte() {
    let mut config = platform();
    let mut l2 = cache("l2");
    l2.config.line_size_bytes = Some(128);
    config.caches = Some(vec![l2]);
    config.memories = Some(vec![memory("top", u64::MAX - 127, 128)]);
    config.validate().unwrap();
}
