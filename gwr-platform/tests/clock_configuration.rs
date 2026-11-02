// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::engine::Engine;
use gwr_platform::Platform;
use gwr_platform::builder::build_clocks;
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;

const NAMED_COMPONENTS: &str = r"
clocks:
  - name: system
    frequency_mhz: 600
  - name: memory
    frequency_mhz: 1200
default_clock: system
memory_maps:
  - name: map
    devices: [hbm0]
processing_elements:
  - name: pe0
    memory_map: map
    clock: system
    config: {}
caches:
  - name: cache0
    clock: memory
    config: {}
fabrics:
  - name: fabric0
    kind: functional
    columns: 2
    rows: 1
    clock: system
    config: {}
memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    clock: memory
    config:
      capacity_bytes: 4096
coherency_managers:
  - name: directory0
    clock: memory
    memory_map: map
    config: {}
";

#[test]
fn accepts_named_clocks_and_component_assignments() {
    let config: PlatformConfig = yaml_serde::from_str(NAMED_COMPONENTS).unwrap();
    config.validate().unwrap();
    let emitted = platform_to_yaml_str(&config).unwrap();
    let value: yaml_serde::Value = yaml_serde::from_str(&emitted).unwrap();
    assert_eq!(value["default_clock"].as_str(), Some("system"));
    assert_eq!(value["clocks"][0]["frequency_mhz"].as_f64(), Some(600.0));
    for (component, clock) in [
        ("processing_elements", "system"),
        ("caches", "memory"),
        ("fabrics", "system"),
        ("memories", "memory"),
        ("coherency_managers", "memory"),
    ] {
        assert_eq!(value[component][0]["clock"].as_str(), Some(clock));
    }
}

fn minimal_config() -> PlatformConfig {
    yaml_serde::from_str(
        "clocks: [{name: system, frequency_mhz: 600}]\ndefault_clock: system\nmemory_maps: []\n",
    )
    .unwrap()
}

#[test]
fn clock_declarations_and_default_are_required() {
    for yaml in [
        "default_clock: system\nmemory_maps: []",
        "clocks: [{name: system, frequency_mhz: 600}]\nmemory_maps: []",
    ] {
        assert!(yaml_serde::from_str::<PlatformConfig>(yaml).is_err());
    }
}

#[test]
fn rejects_empty_clock_list_and_duplicate_names() {
    let mut config = minimal_config();
    config.clocks.clear();
    assert!(config.validate().unwrap_err().to_string().contains("clock"));
    let mut config = minimal_config();
    config.clocks.push(config.clocks[0].clone());
    let error = config.validate().unwrap_err().to_string();
    assert!(
        error.contains("Duplicate clock") && error.contains("system"),
        "{error}"
    );
}

#[test]
fn rejects_nonfinite_or_nonpositive_clock_frequencies() {
    for frequency in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut config = minimal_config();
        config.clocks[0].frequency_mhz = frequency;
        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("system") && error.contains("frequency"),
            "{error}"
        );
    }
}

#[test]
fn rejects_clock_frequencies_with_nonfinite_supported_tick_range_durations() {
    for frequency in [
        1e-306,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        1000.0 / f64::MAX,
        1e-305,
        1e-290,
    ] {
        let mut config = minimal_config();
        config.clocks[0].frequency_mhz = frequency;
        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("system") && error.contains("frequency"),
            "{error}"
        );
        assert!(build_clocks(&mut Engine::default(), &config).is_err());
    }
}

#[test]
fn rejects_clock_frequencies_outside_engine_precision_range() {
    for frequency in [(1.0 / f64::MIN_POSITIVE).next_up(), f64::MAX] {
        let mut config = minimal_config();
        config.clocks[0].frequency_mhz = frequency;
        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("system") && error.contains("frequency"),
            "{error}"
        );
        assert!(build_clocks(&mut Engine::default(), &config).is_err());
    }
}

#[test]
fn rejects_unknown_default_and_component_clocks() {
    let mut config = minimal_config();
    config.default_clock = "missing".to_string();
    let error = config.validate().unwrap_err().to_string();
    assert!(
        error.contains("default") && error.contains("missing"),
        "{error}"
    );

    for (section, fields, label) in [
        (
            "processing_elements",
            "memory_map: map",
            "processing element",
        ),
        ("caches", "", "cache"),
        (
            "fabrics",
            "kind: functional\n    columns: 1\n    rows: 1",
            "fabric",
        ),
        ("memories", "kind: hbm\n    base_address: 0", "memory"),
        ("coherency_managers", "memory_map: map", "coherency manager"),
    ] {
        let yaml = format!(
            "clocks: [{{name: system, frequency_mhz: 600}}]\ndefault_clock: system\nmemory_maps: []\n{section}:\n  - name: device0\n    {fields}\n    clock: missing\n    config: {{capacity_bytes: 4096}}\n"
        );
        // Only memories have a required capacity; keep all other configs empty.
        let yaml = if section == "memories" {
            yaml
        } else {
            yaml.replace("{capacity_bytes: 4096}", "{}")
        };
        let config: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("missing") && error.contains("device0") && error.contains(label),
            "{error}"
        );
    }
}

#[test]
fn validates_clocks_before_effective_model_configuration() {
    let yaml = "clocks: [{name: system, frequency_mhz: 0}]\ndefault_clock: system\nmemory_maps: []\ncaches:\n  - name: cache0\n    config: {num_sets: 0}\n";
    let config: PlatformConfig = yaml_serde::from_str(yaml).unwrap();
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("frequency")
    );
}

#[test]
fn rejects_unknown_manager_clock_before_manager_config_validation() {
    let yaml = NAMED_COMPONENTS
        .replace(
            "name: directory0\n    clock: memory",
            "name: directory0\n    clock: missing",
        )
        .replace(
            "memory_map: map\n    config: {}",
            "memory_map: map\n    config: {operations_per_tick: 0}",
        );
    let mut engine = Engine::default();
    let error = Platform::from_string(&mut engine, &yaml)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("missing") && error.contains("directory0") && error.contains("clock"),
        "{error}"
    );
    engine.run().unwrap();
}

#[test]
fn round_trips_clock_names_requiring_yaml_quoting() {
    let mut config: PlatformConfig = yaml_serde::from_str(NAMED_COMPONENTS).unwrap();
    let names = [
        "",
        " ",
        "\t\n",
        "null",
        "true",
        "123",
        "clock: # ' \" \\ [x] {y}",
        "line\nbreak\r\t\0",
        "clock\u{0085}\u{2028}\u{2029}",
        "速度",
    ];
    for name in names {
        config.clocks[0].name = name.to_string();
        config.default_clock = name.to_string();
        config.processing_elements.as_mut().unwrap()[0].clock = Some(name.to_string());
        config.caches.as_mut().unwrap()[0].clock = Some(name.to_string());
        config.fabrics.as_mut().unwrap()[0].clock = Some(name.to_string());
        config.memories.as_mut().unwrap()[0].clock = Some(name.to_string());
        config.coherency_managers.as_mut().unwrap()[0].clock = Some(name.to_string());
        config.validate().unwrap();
        let yaml = platform_to_yaml_str(&config).unwrap();
        let parsed: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
        assert_eq!(parsed.clocks, config.clocks);
        assert_eq!(parsed.default_clock, name);
        assert_eq!(
            parsed.processing_elements.unwrap()[0].clock.as_deref(),
            Some(name)
        );
        assert_eq!(parsed.caches.unwrap()[0].clock.as_deref(), Some(name));
        assert_eq!(parsed.fabrics.unwrap()[0].clock.as_deref(), Some(name));
        assert_eq!(parsed.memories.unwrap()[0].clock.as_deref(), Some(name));
        assert_eq!(
            parsed.coherency_managers.unwrap()[0].clock.as_deref(),
            Some(name)
        );
    }
}

#[test]
fn emitted_omitted_assignments_still_use_the_default() {
    let yaml = NAMED_COMPONENTS
        .replace("    clock: system\n", "")
        .replace("    clock: memory\n", "");
    let config: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
    config.validate().unwrap();
    let emitted = platform_to_yaml_str(&config).unwrap();
    let value: yaml_serde::Value = yaml_serde::from_str(&emitted).unwrap();
    assert!(value["coherency_managers"][0].get("clock").is_none());
    let parsed: PlatformConfig = yaml_serde::from_str(&emitted).unwrap();
    assert!(parsed.caches.unwrap()[0].clock.is_none());
    assert_eq!(parsed.default_clock, "system");
}

#[test]
fn generated_platform_declares_a_1000_mhz_default_clock() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("platform.yaml");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_gen-fabric"))
        .arg("--out")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let yaml = std::fs::read_to_string(output).unwrap();
    let config: PlatformConfig = yaml_serde::from_str(&yaml).unwrap();
    config.validate().unwrap();
    let default = config
        .clocks
        .iter()
        .find(|clock| clock.name == config.default_clock)
        .unwrap();
    assert_eq!(default.frequency_mhz, 1000.0);
}
