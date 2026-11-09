// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;

fn source(mode: &str) -> String {
    format!(
        r"
memory_maps:
 - name: local
   devices: [m0, m1]
   address_mapping:
    mode: {mode}
    stripe_bytes: 64
    groups: [{{name: hbm, devices: [m1, m0]}}]
memories:
 - {{name: m0, kind: hbm, base_address: 4096, config: {{capacity_bytes: 256}}}}
 - {{name: m1, kind: hbm, base_address: 4352, config: {{capacity_bytes: 256}}}}
"
    )
}

#[test]
fn policy_roundtrips_in_both_modes() {
    for mode in ["contiguous", "striped"] {
        let config: PlatformConfig = yaml_serde::from_str(&source(mode)).unwrap();
        config.validate().unwrap();
        let emitted = platform_to_yaml_str(&config).unwrap();
        let rebuilt: PlatformConfig = yaml_serde::from_str(&emitted).unwrap();
        rebuilt.validate().unwrap();
        assert!(emitted.contains(mode));
        assert_eq!(
            rebuilt.memory_maps[0]
                .address_mapping
                .as_ref()
                .unwrap()
                .groups[0]
                .devices,
            ["m1", "m0"]
        );
    }
}

#[test]
fn invalid_groups_fail_even_when_disabled() {
    for mode in ["contiguous", "striped"] {
        for (old, new) in [
            ("stripe_bytes: 64", "stripe_bytes: 0"),
            ("[m1, m0]", "[m1, m1]"),
            ("[m1, m0]", "[missing, m0]"),
            ("base_address: 4352", "base_address: 4608"),
            ("name: hbm", "name: ''"),
        ] {
            let config: PlatformConfig =
                yaml_serde::from_str(&source(mode).replace(old, new)).unwrap();
            assert!(
                config.validate().is_err(),
                "accepted {old} -> {new} in {mode}"
            );
        }
    }
}

#[test]
fn unknown_policy_and_fields_fail() {
    assert!(yaml_serde::from_str::<PlatformConfig>(&source("random")).is_err());
    assert!(
        yaml_serde::from_str::<PlatformConfig>(
            &source("striped").replace("stripe_bytes: 64", "stripe_bytes: 64\n    surprise: true")
        )
        .is_err()
    );
}

#[test]
fn nested_caches_and_dma_validate_the_selected_map() {
    let extra = r"
operation_classes:
 add: {operations: [add], arity: 2}
processing_elements:
 - name: p
   memory_map: local
   dma_memory_map: local
   config: {lsu_access_bytes: 64, compute: {engines: {cpu: {capabilities: [{class: add, ops_per_tick: 8}]}}}}
caches:
 - {name: c1, config: {line_size_bytes: 64}}
 - {name: c2, config: {line_size_bytes: 128}}
connections:
 - {connect: [pe.p, cache.c1.dev]}
 - {connect: [cache.c1.mem, 'null']}
 - {connect: [pe.p.dma, cache.c2.dev]}
 - {connect: [cache.c2.mem, 'null']}
";
    for mode in ["contiguous", "striped"] {
        let config: PlatformConfig = yaml_serde::from_str(&(source(mode) + extra)).unwrap();
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("stripe") && error.contains("c2"), "{error}");
        let config: PlatformConfig = yaml_serde::from_str(
            &(source(mode).replace("stripe_bytes: 64", "stripe_bytes: 128") + extra),
        )
        .unwrap();
        config.validate().unwrap();
    }
}
