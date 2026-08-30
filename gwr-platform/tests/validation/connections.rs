// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use super::common::*;

#[test]
fn rejects_a_connection_with_one_endpoint() {
    let mut config = platform();
    config.connections = Some(vec![ConnectSection {
        connect: vec!["pe.pe0".to_string()],
    }]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Invalid 'connect'"));
}

#[test]
fn rejects_a_connection_with_three_endpoints() {
    let mut config = platform();
    config.connections = Some(vec![ConnectSection {
        connect: vec![
            "pe.pe0".to_string(),
            "pe.pe1".to_string(),
            "pe.pe2".to_string(),
        ],
    }]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Invalid 'connect'"));
}

#[test]
fn rejects_an_unknown_connection_endpoint() {
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &[])];
    config.processing_elements = Some(vec![processing_element("pe0", "mm0")]);
    config.connections = Some(vec![connection("pe.pe0", "mem.missing")]);

    assert_eq!(
        config.validate().unwrap_err().to_string(),
        "Connection 'pe.pe0' -> 'mem.missing': No Memory 'missing'"
    );
}

#[test]
fn rejects_a_direct_connection_between_processing_elements() {
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &[])];
    config.processing_elements = Some(vec![
        processing_element("pe0", "mm0"),
        processing_element("pe1", "mm0"),
    ]);
    config.connections = Some(vec![connection("pe.pe0", "pe.pe1")]);

    assert_eq!(
        config.validate().unwrap_err().to_string(),
        "Connection 'pe.pe0' -> 'pe.pe1': Cannot connect a PE directly to a PE"
    );
}

#[test]
fn rejects_a_port_used_by_two_connections() {
    let mut config = platform();
    config.memory_maps = vec![memory_map("mm0", &["hbm0", "hbm1"])];
    config.processing_elements = Some(vec![processing_element("pe0", "mm0")]);
    config.memories = Some(vec![memory("hbm0", 0, 1024), memory("hbm1", 1024, 1024)]);
    config.connections = Some(vec![
        connection("pe.pe0", "mem.hbm0"),
        connection("pe.pe0", "mem.hbm1"),
    ]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Port 'pe.pe0' is connected more than once"));
}

#[test]
fn rejects_a_fabric_with_only_one_port() {
    let mut fabric = fabric("fabric0");
    fabric.config.fabric_ports_per_node = None;
    let mut config = platform();
    config.fabrics = Some(vec![fabric]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Fabric 'fabric0': has 1 populated port"));
}

#[test]
fn accepts_an_unconnected_fabric_with_two_ports() {
    let mut config = platform();
    config.fabrics = Some(vec![fabric("fabric0")]);

    config.validate().unwrap();
}

#[test]
fn rejects_fabric_port_count_overflow() {
    let mut fabric = fabric("fabric0");
    fabric.kind = FabricKind::Routed;
    fabric.columns = usize::MAX;
    fabric.rows = 2;
    fabric.config.fabric_ports_per_node = Some(1);
    let mut config = platform();
    config.fabrics = Some(vec![fabric]);

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("Fabric 'fabric0': maximum port count overflows"));
}
