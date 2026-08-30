// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::*;

#[test]
fn rejects_duplicate_device_names() {
    let mut duplicate_pes = platform();
    duplicate_pes.memory_maps = vec![memory_map("mm0", &[])];
    duplicate_pes.processing_elements = Some(vec![
        processing_element("pe0", "mm0"),
        processing_element("pe0", "mm0"),
    ]);

    let mut duplicate_memories = platform();
    duplicate_memories.memories = Some(vec![memory("mem0", 0, 1), memory("mem0", 1, 1)]);

    let mut shared_name = platform();
    shared_name.memory_maps = vec![memory_map("mm0", &[])];
    shared_name.processing_elements = Some(vec![processing_element("dev0", "mm0")]);
    shared_name.memories = Some(vec![memory("dev0", 0, 1)]);

    for config in [duplicate_pes, duplicate_memories, shared_name] {
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Duplicate device name"));
    }
}

#[test]
fn rejects_duplicate_resource_names() {
    let mut duplicate_memory_maps = platform();
    duplicate_memory_maps.memory_maps =
        vec![memory_map("duplicate", &[]), memory_map("duplicate", &[])];

    let mut duplicate_caches = platform();
    duplicate_caches.caches = Some(vec![cache("duplicate"), cache("duplicate")]);

    let mut duplicate_fabrics = platform();
    duplicate_fabrics.fabrics = Some(vec![fabric("duplicate"), fabric("duplicate")]);

    for (config, expected) in [
        (duplicate_memory_maps, "Duplicate memory map name duplicate"),
        (duplicate_caches, "Duplicate cache name duplicate"),
        (duplicate_fabrics, "Duplicate fabric name duplicate"),
    ] {
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains(expected));
    }
}

#[test]
fn rejects_a_processing_element_as_a_memory_map_device() {
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &["pe0"])];
    config.processing_elements = Some(vec![processing_element("pe0", "mm0")]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Unknown memory 'pe0' in memory map 'mm0'"));
}

#[test]
fn rejects_a_memory_as_a_processing_element_endpoint() {
    let mut config = platform();
    config.memories = Some(vec![memory("mem0", 0, 1024)]);
    config.connections = Some(vec![connection("pe.mem0", "mem.mem0")]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("No PE 'mem0'"));
}

#[test]
fn rejects_duplicate_memories_in_one_memory_map() {
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &["hbm0", "hbm0"])];
    config.memories = Some(vec![memory("hbm0", 0, 1024)]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Duplicate memory 'hbm0' in memory map 'mm0'"));
}

#[test]
fn rejects_an_unknown_processing_element_memory_map() {
    let mut config = platform();
    config.processing_elements = Some(vec![processing_element("pe0", "missing")]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Unknown memory map 'missing' for processing element 'pe0'"));
}

#[test]
fn rejects_an_unknown_memory_in_a_memory_map() {
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &["hbm0", "missing"])];
    config.memories = Some(vec![memory("hbm0", 0, 1024)]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Unknown memory 'missing' in memory map 'mm0'"));
}

#[test]
#[should_panic(expected = "Started without dispatcher")]
fn processing_element_requires_a_dispatcher() {
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
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
      capacity_bytes: 1024
connections:
  - connect: [pe.pe0, mem.hbm0]
",
    )
    .unwrap();

    engine.run().unwrap();
}
