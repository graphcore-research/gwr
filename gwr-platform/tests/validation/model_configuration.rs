// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::*;

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
    for (name, value, expected) in [
        ("adds", 0.0, "add throughput"),
        ("muls", -1.0, "multiply throughput"),
        ("compares", f64::NAN, "comparison throughput"),
        ("adds", f64::INFINITY, "add throughput"),
    ] {
        let mut pe = processing_element("pe0", "mm0");
        match name {
            "adds" => pe.config.adds_per_tick = Some(value),
            "muls" => pe.config.muls_per_tick = Some(value),
            "compares" => pe.config.compares_per_tick = Some(value),
            _ => unreachable!(),
        }
        let mut config = platform();
        config.memory_maps = vec![memory_map("mm0", &[])];
        config.processing_elements = Some(vec![pe]);

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Processing element 'pe0'"));
        assert!(error.contains(expected));
    }
}

#[test]
fn accepts_positive_fractional_processing_element_throughput() {
    let mut pe = processing_element("pe0", "mm0");
    pe.config.adds_per_tick = Some(0.25);
    pe.config.muls_per_tick = Some(0.5);
    pe.config.compares_per_tick = Some(0.75);
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

    for (cache, expected) in [
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
