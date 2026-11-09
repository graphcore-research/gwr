// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;

#[test]
fn platform_catalog_round_trips_and_validates_referenced_class() {
    let yaml = r#"
operation_classes:
  trig: { operations: [sin, cos, tan], arity: 1 }
memory_maps:
  - { name: mm, devices: [mem] }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      compute:
        engines:
          vector:
            capabilities:
              - { class: trig, input_dtypes: [[fp32]], ops_per_tick: 32 }
memories:
  - { name: mem, kind: hbm, base_address: 0, config: { capacity_bytes: 1MiB } }
connections:
  - { connect: [pe.pe0, mem.mem] }
"#;
    let platform: PlatformConfig = yaml_serde::from_str(yaml).unwrap();
    platform.validate().unwrap();
    let emitted = platform_to_yaml_str(&platform).unwrap();
    let decoded: PlatformConfig = yaml_serde::from_str(&emitted).unwrap();
    assert!(decoded.operation_classes.contains_key("trig"));
    decoded.validate().unwrap();
}
