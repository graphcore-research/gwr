// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_platform::types::PlatformConfig;

const DEFAULT_LINE_SIZE_BYTES: usize = 32;
const BACKING_MEMORY_CAPACITY_BYTES: u64 = 1536;

#[test]
fn manager_processing_rate_survives_yaml_roundtrip_without_a_line_size() {
    let mut cfg = create_coherent_platform_config(DEFAULT_LINE_SIZE_BYTES, DEFAULT_LINE_SIZE_BYTES);
    cfg.coherency_managers.as_mut().unwrap()[0].config =
        yaml_serde::from_str("operations_per_tick: 2").unwrap();
    cfg.validate().unwrap();
    let yaml = gwr_platform::yaml::platform_to_yaml_str(&cfg).unwrap();
    let roundtrip: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
    let managers = roundtrip.coherency_managers.unwrap();
    assert_eq!(managers[0].config.operations_per_tick, Some(2));
    assert_eq!(managers[0].config.line_size_bytes, None);
}

#[test]
fn zero_manager_processing_rate_is_rejected() {
    let mut cfg = create_coherent_platform_config(DEFAULT_LINE_SIZE_BYTES, DEFAULT_LINE_SIZE_BYTES);
    cfg.coherency_managers.as_mut().unwrap()[0].config =
        yaml_serde::from_str("operations_per_tick: 0").unwrap();
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("operations per tick")
    );
}

#[test]
fn coherent_platform_accepts_matching_non_power_of_two_line_sizes() {
    create_coherent_platform_config(24, 24).validate().unwrap();
}

#[test]
fn coherent_platform_rejects_mismatched_line_sizes() {
    // Validate caches and managers with different line sizes.
    let error = create_coherent_platform_config(DEFAULT_LINE_SIZE_BYTES, 64)
        .validate()
        .unwrap_err()
        .to_string();

    // Report the coherence line-size mismatch.
    assert!(error.contains("line size"), "{error}");
}

#[test]
fn coherent_platform_rejects_zero_manager_line_size() {
    assert!(
        create_coherent_platform_config(DEFAULT_LINE_SIZE_BYTES, 0)
            .validate()
            .is_err()
    );
}

#[test]
fn coherent_platform_rejects_misaligned_manager_memory_boundaries() {
    // Accept aligned backing ranges.
    assert!(create_coherent_platform_config(64, 64).validate().is_ok());

    // Reject backing memory whose size is not a multiple of the line size.
    assert!(create_coherent_platform_config(40, 40).validate().is_err());
}

#[test]
fn coherent_platform_rejects_cache_hierarchies() {
    // Connect an additional upper cache to the coherent cache.
    let mut cfg = create_coherent_platform_config(DEFAULT_LINE_SIZE_BYTES, DEFAULT_LINE_SIZE_BYTES);
    let mut upper = cfg.caches.as_ref().unwrap()[0].clone();
    upper.name = "upper".into();
    upper.coherency_managers = None;
    cfg.caches.as_mut().unwrap().push(upper);
    cfg.connections = Some(vec![gwr_platform::types::ConnectSection {
        connect: vec!["cache.upper.mem".into(), "cache.cache.dev".into()],
    }]);

    // Reject the unsupported coherent hierarchy.
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("hierarchical coherence")
    );
}

#[test]
fn coherent_cache_rejects_direct_memory_connection() {
    // Connect the coherent cache directly to backing memory.
    let mut cfg = create_coherent_platform_config(DEFAULT_LINE_SIZE_BYTES, DEFAULT_LINE_SIZE_BYTES);
    cfg.connections = Some(vec![gwr_platform::types::ConnectSection {
        connect: vec!["cache.cache.mem".into(), "mem.memory".into()],
    }]);

    // Require coherence traffic to pass through a fabric.
    assert!(cfg.validate().unwrap_err().to_string().contains("Fabric"));
}

#[test]
fn independently_selected_managers_cannot_own_the_same_backing_memory() {
    // Add a second directory over the same backing memory.
    let mut cfg = create_coherent_platform_config(DEFAULT_LINE_SIZE_BYTES, DEFAULT_LINE_SIZE_BYTES);
    let mut other_manager = cfg.coherency_managers.as_ref().unwrap()[0].clone();
    other_manager.name = "other_directory".into();
    cfg.coherency_managers.as_mut().unwrap().push(other_manager);

    // Give another cache its own independently selected directory.
    let mut other_cache = cfg.caches.as_ref().unwrap()[0].clone();
    other_cache.name = "other_cache".into();
    other_cache.coherency_managers = Some(vec!["other_directory".into()]);
    cfg.caches.as_mut().unwrap().push(other_cache);

    // Reject overlapping ownership of backing memory.
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("multiple coherency managers")
    );
}

#[test]
fn upstream_pe_memory_map_must_be_covered_by_selected_managers() {
    // Add memory that the selected directory does not cover.
    let mut cfg = create_coherent_platform_config(DEFAULT_LINE_SIZE_BYTES, DEFAULT_LINE_SIZE_BYTES);
    let mut other = cfg.memories.as_ref().unwrap()[0].clone();
    other.name = "other".into();
    other.base_address = BACKING_MEMORY_CAPACITY_BYTES;
    cfg.memories.as_mut().unwrap().push(other);

    // Expose both memories through the processing element memory map.
    cfg.memory_maps.push(gwr_platform::types::MemoryMapSection {
        name: "pe_map".into(),
        devices: vec!["memory".into(), "other".into()],
    });
    cfg.processing_elements = Some(vec![gwr_platform::types::ProcessingElementSection {
        name: "pe".into(),
        clock: None,
        memory_map: "pe_map".into(),
        config: yaml_serde::from_str("{}").unwrap(),
    }]);

    // Route that processing element through the coherent cache.
    cfg.connections = Some(vec![gwr_platform::types::ConnectSection {
        connect: vec!["pe.pe".into(), "cache.cache.dev".into()],
    }]);

    // Reject the uncovered upstream memory region.
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("no coherency manager for Memory 'other'")
    );
}

fn create_coherent_platform_config(
    cache_line_size_bytes: usize,
    manager_line_size_bytes: usize,
) -> PlatformConfig {
    yaml_serde::from_str(&format!(
        r"
clocks:
  - name: default
    frequency_mhz: 1000
default_clock: default

memory_maps:
  - name: backing
    devices: [memory]
memories:
  - name: memory
    kind: hbm
    base_address: 0
    config: {{capacity_bytes: {BACKING_MEMORY_CAPACITY_BYTES}}}
caches:
  - name: cache
    coherency_managers: [directory]
    config: {{line_size_bytes: {cache_line_size_bytes}, max_inflight_parents: 1, max_inflight_fills: 1}}
coherency_managers:
  - name: directory
    memory_map: backing
    config: {{line_size_bytes: {manager_line_size_bytes}}}
"
    ))
    .unwrap()
}
