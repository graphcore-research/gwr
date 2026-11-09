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
      engines:
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
fn rejects_invalid_processing_element_configuration() {
    let mut no_requests = processing_element("pe0", "mm0");
    no_requests.config.num_active_requests = Some(0);
    let mut no_access_bytes = processing_element("pe0", "mm0");
    no_access_bytes.config.lsu_access_bytes = Some(0);

    for (pe, expected) in [
        (no_requests, "LSU request-slot count"),
        (no_access_bytes, "LSU access size"),
    ] {
        let mut config = platform();
        config.memory_maps = vec![memory_map("mm0", &[])];
        config.processing_elements = Some(vec![pe]);
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Processing element 'pe0'"));
        assert!(error.contains(expected));
    }
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
        compute.engines.get_mut("alu").unwrap().capabilities[capability].ops_per_tick = value;
        pe.config.compute = Some(gwr_platform::types::ComputeSelection::Inline(compute));
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
        .engines
        .get_mut("alu")
        .unwrap()
        .capabilities
        .iter_mut()
        .zip([0.25, 0.5, 0.75])
    {
        capability.ops_per_tick = rate;
    }
    pe.config.compute = Some(gwr_platform::types::ComputeSelection::Inline(compute));
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &[])];
    config.processing_elements = Some(vec![pe]);

    config.validate().unwrap();
}

#[test]
fn rejects_invalid_cache_configuration() {
    let mut no_line_bytes = cache("cache0");
    no_line_bytes.config.line_size_bytes = Some(0);
    let mut no_bandwidth = cache("cache0");
    no_bandwidth.config.bw_bytes_per_tick = Some(0);
    let mut no_sets = cache("cache0");
    no_sets.config.num_sets = Some(0);
    let mut no_ways = cache("cache0");
    no_ways.config.num_ways = Some(0);

    let mut no_parents = cache("cache0");
    no_parents.config.max_inflight_parents = Some(0);
    let mut no_fills = cache("cache0");
    no_fills.config.max_inflight_fills = Some(0);

    for (cache, expected) in [
        (no_parents, "admission bounds"),
        (no_fills, "admission bounds"),
        (no_line_bytes, "line size"),
        (no_bandwidth, "bandwidth"),
        (no_sets, "set count"),
        (no_ways, "way count"),
    ] {
        let mut config = platform();
        config.caches = Some(vec![cache]);
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Cache 'cache0'"));
        assert!(error.contains(expected));
    }
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
fn validates_large_fabric_without_materialising_ports() {
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
