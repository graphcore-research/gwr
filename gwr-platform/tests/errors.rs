// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::test_helpers::start_test;
use gwr_platform::Platform;

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
