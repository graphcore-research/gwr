// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::test_helpers::start_test;
use gwr_platform::Platform;

fn assert_error_contains<T: std::fmt::Debug, E: std::fmt::Display>(
    result: Result<T, E>,
    expected: &str,
) {
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains(expected),
        "expected '{expected}' in '{err}'"
    );
}

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
fn duplicate_pe_name() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
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
        ),
        "Duplicate device",
    );
}

#[test]
fn duplicate_mem_name() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
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
        ),
        "Duplicate device",
    );
}

#[test]
fn duplicate_device_name() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
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
        ),
        "Duplicate device",
    );
}

#[test]
fn duplicate_coherency_manager_name() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
        Platform::from_string(
            &engine,
            &clock,
            "
memory_maps:
  - name: mm0
    devices: []

coherency_managers:
  - name: cm0
    memory_map: mm0
    config:
  - name: cm0
    memory_map: mm0
    config:
",
        ),
        "Duplicate device",
    );
}

#[test]
fn unknown_coherency_manager_config_field_is_rejected() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let err = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices: []

coherency_managers:
  - name: cm0
    memory_map: mm0
    config:
      line_siz_bytes: 64
",
    )
    .unwrap_err();

    assert!(format!("{err}").contains("unknown field `line_siz_bytes`"));
}

#[test]
fn unknown_coherency_manager_on_cache() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
        Platform::from_string(
            &engine,
            &clock,
            "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0

caches:
  - name: c0
    memory_map: mm0
    coherency_manager: cm_missing
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    capacity_bytes: 16GiB
",
        ),
        "Unknown coherency manager 'cm_missing'",
    );
}

#[test]
fn unknown_coherency_manager_in_cache_list() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
        Platform::from_string(
            &engine,
            &clock,
            "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0

caches:
  - name: c0
    memory_map: mm0
    coherency_managers:
      - cm_missing
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    capacity_bytes: 16GiB
",
        ),
        "Unknown coherency manager 'cm_missing'",
    );
}

#[test]
fn cache_cannot_set_single_and_multiple_coherency_managers() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
        Platform::from_string(
            &engine,
            &clock,
            "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0

caches:
  - name: c0
    memory_map: mm0
    coherency_manager: cm0
    coherency_managers:
      - cm0
    config:

coherency_managers:
  - name: cm0
    memory_map: mm0
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    capacity_bytes: 16GiB
",
        ),
        "cannot set both coherency_manager and coherency_managers",
    );
}

#[test]
fn unknown_memory_map_on_cache() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
        Platform::from_string(
            &engine,
            &clock,
            "
memory_maps:
  - name: mm0
    devices:
      - name: hbm0

caches:
  - name: c0
    memory_map: mm_missing
    config:

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    capacity_bytes: 16GiB
",
        ),
        "Unknown memory map 'mm_missing'",
    );
}

#[test]
fn no_dispatcher() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let _platform = Platform::from_string(
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
    assert_error_contains(engine.run(), "Started without dispatcher");
}

#[test]
fn unknown_memory_in_memory_map() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
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
        ),
        "Unknown memory 'hbm_missing'",
    );
}

#[test]
fn invalid_connect_1() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
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
        ),
        "Invalid 'connect'",
    );
}

#[test]
fn invalid_connect_3() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    assert_error_contains(
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
        ),
        "Invalid 'connect'",
    );
}
