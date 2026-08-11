// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::test_helpers::start_test;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;

#[test]
fn unknown_top_level_field_is_rejected() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let err = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps: []
processing_elementz: []
",
    )
    .unwrap_err();

    assert!(format!("{err}").contains("unknown field `processing_elementz`"));
}

#[test]
fn unknown_pe_config_field_is_rejected() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let err = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices: []

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
      lsu_acess_bytes: 32
",
    )
    .unwrap_err();

    assert!(format!("{err}").contains("unknown field `lsu_acess_bytes`"));
}

#[test]
fn defaults_pe_config_anchor_is_allowed() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices: []

defaults:
  pe_config: &default_pe_config
    lsu_access_bytes: 32

processing_elements:
  - name: pe0
    memory_map: mm0
    config: *default_pe_config
",
    )
    .unwrap();

    assert_eq!(platform.num_pes(), 1);
}

#[test]
fn defaults_pe_config_anchor_is_type_checked() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let err = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps: []

defaults:
  pe_config:
    lsu_acess_bytes: 32
",
    )
    .unwrap_err();

    assert!(format!("{err}").contains("unknown field `lsu_acess_bytes`"));
}

#[test]
#[should_panic(expected = "Duplicate device")]
fn duplicate_pe_name() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices: []

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
  - name: pe0
    memory_map: mm0
    config:
",
    )
    .unwrap();
}

#[test]
#[should_panic(expected = "Duplicate device")]
fn duplicate_mem_name() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    Platform::from_string(
        &engine,
        &clock,
        "
memory_maps: []

memories:
  - name: mem0
    kind: hbm
    base_address: 0
    capacity_bytes: 0
  - name: mem0
    kind: hbm
    base_address: 0
    capacity_bytes: 0
",
    )
    .unwrap();
}

#[test]
#[should_panic(expected = "Duplicate device")]
fn duplicate_device_name() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices: []

processing_elements:
  - name: dev0
    memory_map: mm0
    config:
memories:
  - name: dev0
    kind: hbm
    base_address: 0
    capacity_bytes: 0
",
    )
    .unwrap();
}

#[test]
fn platform_config_validate_rejects_duplicate_device_names() {
    let platform: PlatformConfig = serde_yaml::from_str(
        "
memory_maps:
  - name: mm0
    devices: []

processing_elements:
  - name: dev0
    memory_map: mm0
    config:

memories:
  - name: dev0
    kind: hbm
    base_address: 0
    capacity_bytes: 0
",
    )
    .unwrap();

    let err = platform.validate().unwrap_err();
    assert!(format!("{err}").contains("Duplicate device name dev0"));
}

#[test]
fn memory_ending_at_top_of_physical_address_space_is_valid() {
    let platform: PlatformConfig = serde_yaml::from_str(
        "
memory_maps: []
memories:
  - name: top
    kind: hbm
    base_address: 18446744073709551614
    capacity_bytes: 1
",
    )
    .unwrap();

    platform.validate().unwrap();
}

#[test]
fn memory_range_rejects_exclusive_end_overflow() {
    let platform: PlatformConfig = serde_yaml::from_str(
        "
memory_maps: []
memories:
  - name: overflowing
    kind: hbm
    base_address: 18446744073709551615
    capacity_bytes: 1
",
    )
    .unwrap();

    let err = platform.validate().unwrap_err();
    assert!(format!("{err}").contains("Memory 'overflowing' range overflows"));
}

#[test]
fn memory_map_rejects_zero_capacity_memory() {
    let platform: PlatformConfig = serde_yaml::from_str(
        "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 0
",
    )
    .unwrap();

    let err = platform.validate().unwrap_err();
    assert!(format!("{err}").contains("Memory 'hbm0' in memory map 'mm0' has zero capacity"));
}

#[test]
fn memory_map_rejects_duplicate_memory() {
    let platform: PlatformConfig = serde_yaml::from_str(
        "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0
      - name: hbm0
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1024
",
    )
    .unwrap();

    let err = platform.validate().unwrap_err();
    assert!(format!("{err}").contains("Duplicate memory 'hbm0' in memory map 'mm0'"));
}

#[test]
fn overlapping_physical_memories_are_rejected() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let err = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps: []
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1024
  - name: hbm1
    kind: hbm
    base_address: 512
    capacity_bytes: 1024
",
    )
    .unwrap_err();

    let message = format!("{err}");
    assert!(message.contains("Physical memory ranges overlap"));
    assert!(message.contains("'hbm0' (0x0..0x400)"));
    assert!(message.contains("'hbm1' (0x200..0x600)"));
}

#[test]
fn unknown_pe_memory_map_is_rejected() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let err = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps: []
processing_elements:
  - name: pe0
    memory_map: missing
    config: {}
",
    )
    .unwrap_err();

    assert!(format!("{err}").contains("Unknown memory map 'missing' for processing element 'pe0'"));
}

#[test]
#[should_panic(expected = "Started without dispatcher")]
fn no_dispatcher() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
      num_active_requests: 8
      lsu_access_bytes: 32

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    capacity_bytes: 16GiB

connections:
  - connect:
    - pe.pe0
    - mem.hbm0
",
    )
    .unwrap();
    engine.run().unwrap();
}

#[test]
#[should_panic(expected = "Unknown memory 'hbm_missing'")]
fn unknown_memory_in_memory_map() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0
      - name: hbm_missing

processing_elements:
  - name: pe0
    memory_map: mm0
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1024
",
    )
    .unwrap();
}

#[test]
#[should_panic(expected = "Invalid 'connect'")]
fn invalid_connect_1() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices: []

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
  - name: pe1
    memory_map: mm0
    config:

connections:
  - connect:
    - pe.pe0
",
    )
    .unwrap();
}

#[test]
#[should_panic(expected = "Invalid 'connect'")]
fn invalid_connect_3() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices: []

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
  - name: pe1
    memory_map: mm0
    config:
  - name: pe2
    memory_map: mm0
    config:

connections:
  - connect:
    - pe.pe0
    - pe.pe1
    - pe.pe2
",
    )
    .unwrap();
}

#[test]
fn platform_config_validate_rejects_invalid_connection_endpoint() {
    let platform: PlatformConfig = serde_yaml::from_str(
        "
memory_maps:
  - name: mm0
    devices: []

processing_elements:
  - name: pe0
    memory_map: mm0
    config:

connections:
  - connect:
    - pe.pe0
    - mem.missing
",
    )
    .unwrap();

    let err = platform.validate().unwrap_err();
    assert!(format!("{err}").contains("No Memory 'missing'"));
}

#[test]
fn platform_config_validate_rejects_direct_pe_connections() {
    let platform: PlatformConfig = serde_yaml::from_str(
        "
memory_maps:
  - name: mm0
    devices: []

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
  - name: pe1
    memory_map: mm0
    config:

connections:
  - connect:
    - pe.pe0
    - pe.pe1
",
    )
    .unwrap();

    let err = platform.validate().unwrap_err();
    assert!(format!("{err}").contains("Cannot connect a PE directly to a PE"));
}

#[test]
fn platform_config_validate_rejects_duplicate_connection_port() {
    let platform: PlatformConfig = serde_yaml::from_str(
        "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0
      - name: hbm1

processing_elements:
  - name: pe0
    memory_map: mm0
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    capacity_bytes: 1024
  - name: hbm1
    kind: hbm
    base_address: 1024
    capacity_bytes: 1024

connections:
  - connect:
    - pe.pe0
    - mem.hbm0
  - connect:
    - pe.pe0
    - mem.hbm1
",
    )
    .unwrap();

    let err = platform.validate().unwrap_err();
    assert!(format!("{err}").contains("Port 'pe.pe0' is connected more than once"));
}
