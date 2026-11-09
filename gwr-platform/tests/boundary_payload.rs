// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Task1 schema/configuration proof; no program-runner support is asserted
//! here.
use gwr_platform::types::{BoundaryPayloadServiceSection, PlatformConfig};
use gwr_platform::yaml::platform_to_yaml_str;

fn source(field: &str) -> String {
    format!(
        r#"
memory_maps: [{{name: map, devices: [hbm]}}]
operation_classes:
 add: {{operations: [add], arity: 2}}
processing_elements:
 - name: pe
   memory_map: map
   config:
    {field}
    compute: {{engines: {{alu: {{capabilities: [{{class: add, ops_per_tick: 4}}]}}}}}}
memories:
 - {{name: hbm, kind: hbm, base_address: 0, config: {{capacity_bytes: 4096}}}}
fabrics:
 - {{name: f, kind: functional, columns: 2, rows: 1, config: {{fabric_ports_per_node: 1}}}}
connections:
 - {{connect: [pe.pe, 'fabric.f@(0,0)']}}
 - {{connect: [mem.hbm, 'fabric.f@(1,0)']}}
"#
    )
}
#[test]
fn optional_boundary_payload_roundtrips_and_omission_is_unchanged() {
    let mut base: PlatformConfig = yaml_serde::from_str(&source("")).unwrap();
    let original = platform_to_yaml_str(&base).unwrap();
    assert!(!original.contains("boundary_payload_service"));
    base.processing_elements.as_mut().unwrap()[0]
        .config
        .boundary_payload_service = Some(BoundaryPayloadServiceSection { bytes_per_tick: 4 });
    let yaml = platform_to_yaml_str(&base).unwrap();
    let reparsed: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
    assert_eq!(
        base.processing_elements.as_ref().unwrap()[0].config,
        reparsed.processing_elements.as_ref().unwrap()[0].config
    );
    base.processing_elements.as_mut().unwrap()[0]
        .config
        .boundary_payload_service = None;
    assert_eq!(original, platform_to_yaml_str(&base).unwrap());
}
#[test]
fn boundary_payload_wire_and_native_clock_reject_invalid_configuration() {
    for fields in [
        "bytes_per_tick: 4.0",
        "bytes_per_tick: true",
        "bytes_per_tick: -1",
        "bytes_per_tick: [4]",
        "bytes_per_tick: 4, typo: true",
        "",
    ] {
        assert!(
            yaml_serde::from_str::<PlatformConfig>(&source(&format!(
                "boundary_payload_service: {{{fields}}}"
            )))
            .is_err(),
            "{fields}"
        );
    }
    let mut engine = gwr_engine::engine::Engine::default();
    let clock = engine.default_clock();
    assert!(
        gwr_platform::Platform::from_string(
            &engine,
            &clock,
            &source("boundary_payload_service: {bytes_per_tick: 0}")
        )
        .is_err()
    );
    gwr_platform::Platform::from_string(
        &engine,
        &clock,
        &source("boundary_payload_service: {bytes_per_tick: 4}"),
    )
    .unwrap();
    let other = engine.clock_ghz(2.0);
    assert!(
        gwr_platform::Platform::from_string(
            &engine,
            &other,
            &source("boundary_payload_service: {bytes_per_tick: 4}")
        )
        .is_err()
    );
    // Omitted policy remains valid at the old arbitrary-clock contract.
    gwr_platform::Platform::from_string(&engine, &other, &source("")).unwrap();
}
