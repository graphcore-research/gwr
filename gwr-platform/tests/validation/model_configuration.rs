// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::test_helpers::start_test;
use gwr_platform::Platform;

use super::common::{
    basic_compute, cache, fabric, memory, memory_map, platform, processing_element,
};

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
    compute:
      execution_units:
        alu:
          capabilities: [{ class: add, ops_per_tick: 16 }]

operation_classes:
  add: { operations: [add], arity: 2 }

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
fn default_pe_configuration_rejects_unknown_fields() {
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
fn rejects_zero_processing_element_request_slots() {
    let mut pe = processing_element("pe0", "mm0");
    pe.config.num_active_requests = Some(0);
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &[])];
    config.processing_elements = Some(vec![pe]);
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Processing element 'pe0'"));
    assert!(error.contains("LSU request-slot count"));
}

#[test]
fn rejects_zero_processing_element_access_size() {
    let mut pe = processing_element("pe0", "mm0");
    pe.config.lsu_access_bytes = Some(0);
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &[])];
    config.processing_elements = Some(vec![pe]);
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Processing element 'pe0'"));
    assert!(error.contains("LSU access size"));
}

#[test]
fn rejects_invalid_processing_element_throughput() {
    for (name, value) in [
        ("adds", 0.0),
        ("muls", -1.0),
        ("compares", f64::NAN),
        ("adds", f64::INFINITY),
    ] {
        let mut pe = processing_element("pe0", "mm0");
        let mut compute = basic_compute();
        let capability = match name {
            "adds" => 0,
            "muls" => 1,
            "compares" => 2,
            _ => unreachable!(),
        };
        compute.execution_units.get_mut("alu").unwrap().capabilities[capability].ops_per_tick =
            value;
        pe.config.compute = Some(compute);
        let mut config = platform();
        config.memory_maps = vec![memory_map("mm0", &[])];
        config.processing_elements = Some(vec![pe]);

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Processing element 'pe0'"));
        assert!(error.contains("compute throughput"));
    }
}

#[test]
fn accepts_positive_fractional_processing_element_throughput() {
    let mut pe = processing_element("pe0", "mm0");
    let mut compute = basic_compute();
    for (capability, rate) in compute
        .execution_units
        .get_mut("alu")
        .unwrap()
        .capabilities
        .iter_mut()
        .zip([0.25, 0.5, 0.75])
    {
        capability.ops_per_tick = rate;
    }
    pe.config.compute = Some(compute);
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &[])];
    config.processing_elements = Some(vec![pe]);

    config.validate().unwrap();
}

#[test]
fn rejects_zero_cache_admission_bounds() {
    let mut no_parents = cache("cache0");
    no_parents.config.max_inflight_parents = Some(0);
    let mut no_fills = cache("cache0");
    no_fills.config.max_inflight_fills = Some(0);
    for cache in [no_parents, no_fills] {
        let mut config = platform();
        config.caches = Some(vec![cache]);
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Cache 'cache0'"));
        assert!(error.contains("admission bounds"));
    }
}

#[test]
fn rejects_zero_cache_line_size() {
    let mut cache = cache("cache0");
    cache.config.line_size_bytes = Some(0);
    let mut config = platform();
    config.caches = Some(vec![cache]);
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Cache 'cache0'"));
    assert!(error.contains("line size"));
}

#[test]
fn rejects_zero_cache_bandwidth() {
    let mut cache = cache("cache0");
    cache.config.bw_bytes_per_tick = Some(0);
    let mut config = platform();
    config.caches = Some(vec![cache]);
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Cache 'cache0'"));
    assert!(error.contains("bandwidth"));
}

#[test]
fn rejects_zero_cache_set_count() {
    let mut cache = cache("cache0");
    cache.config.num_sets = Some(0);
    let mut config = platform();
    config.caches = Some(vec![cache]);
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Cache 'cache0'"));
    assert!(error.contains("set count"));
}

#[test]
fn rejects_zero_cache_way_count() {
    let mut cache = cache("cache0");
    cache.config.num_ways = Some(0);
    let mut config = platform();
    config.caches = Some(vec![cache]);
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Cache 'cache0'"));
    assert!(error.contains("way count"));
}

#[test]
fn rejects_invalid_fabric_transport_configuration() {
    let mut no_receive_buffer = fabric("fabric0");
    no_receive_buffer.config.rx_buffer_bytes = Some(0);
    let mut no_transmit_buffer = fabric("fabric0");
    no_transmit_buffer.config.tx_buffer_bytes = Some(0);
    let mut no_link_rate = fabric("fabric0");
    no_link_rate.config.port_bits_per_tick = Some(0);

    for (fabric, expected) in [
        (no_receive_buffer, "receive buffer size"),
        (no_transmit_buffer, "transmit buffer size"),
        (no_link_rate, "link rate"),
    ] {
        let mut config = platform();
        config.fabrics = Some(vec![fabric]);
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Fabric 'fabric0'"));
        assert!(error.contains(expected));
    }
}

#[test]
fn accepts_large_fabric_dimensions() {
    let mut large_fabric = fabric("fabric0");
    large_fabric.columns = 1_000_000_000;
    large_fabric.rows = 1;
    large_fabric.config.fabric_ports_per_node = Some(1);
    let mut config = platform();
    config.fabrics = Some(vec![large_fabric]);

    config.validate().unwrap();
}

#[test]
fn rejects_zero_memory_bandwidth() {
    let mut hbm = memory("hbm0", 0, 1024);
    hbm.config.bw_bytes_per_tick = Some(0);
    let mut config = platform();
    config.memories = Some(vec![hbm]);
    let error = config.validate().unwrap_err().to_string();

    assert!(error.contains("Memory 'hbm0': bandwidth must be greater than zero"));
}
