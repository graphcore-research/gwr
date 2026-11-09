// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;

const LIMITS: &str = "{max_compute_tiles: 1, max_load_chunks: 1, max_store_tiles: 1, transfer_port: lsu, accumulator_storage: shared_sram}";

#[test]
fn block_execution_roundtrips_when_it_is_the_only_explicit_configuration() {
    let source = format!(
        "memory_maps: [{{name: mm, devices: []}}]\nprocessing_elements:\n  - name: p0\n    memory_map: mm\n    config: &cfg {{block_execution: {LIMITS}}}\n  - name: p1\n    memory_map: mm\n    config: *cfg\n"
    );
    let different = LIMITS.replace('}', ", local_memory_service: {bytes_per_tick: 7}}");
    let source = format!(
        "{source}  - name: p2\n    memory_map: mm\n    config: {{block_execution: {different}}}\n"
    );
    let config: PlatformConfig = yaml_serde::from_str(&source).unwrap();
    let written = platform_to_yaml_str(&config).unwrap();
    assert!(written.contains("block_execution:"));
    assert!(written.contains("config: *pe_config_0"));
    assert!(written.contains("config: &pe_config_1"));
    let reparsed: PlatformConfig = yaml_serde::from_str(&written).unwrap();
    for (before, after) in config
        .processing_elements
        .unwrap()
        .iter()
        .zip(reparsed.processing_elements.unwrap().iter())
    {
        assert_eq!(before.config, after.config);
    }
}

fn platform_with_limits(limits: Option<&str>) -> PlatformConfig {
    let source = include_str!("../../gwr-timetable/examples/compute_region_platform.yaml");
    let source = match limits {
        Some(limits) => source.replace(
            "      sram_bytes: 256",
            &format!("      block_execution: {limits}\n      sram_bytes: 256"),
        ),
        None => source.to_owned(),
    };
    yaml_serde::from_str(&source).unwrap()
}

#[test]
fn block_execution_defaults_and_invalid_limits() {
    let absent = platform_with_limits(None);
    let config = absent.processing_elements.as_ref().unwrap()[0]
        .effective_config(&absent.operation_classes, &absent.compute_profiles)
        .unwrap();
    assert!(config.block_execution.is_none());
    let selected = platform_with_limits(Some(LIMITS));
    selected.validate().unwrap();
    let config = selected.processing_elements.as_ref().unwrap()[0]
        .effective_config(&selected.operation_classes, &selected.compute_profiles)
        .unwrap();
    assert_eq!(config.block_execution.unwrap().max_load_chunks, 1);
    for field in ["max_compute_tiles", "max_load_chunks", "max_store_tiles"] {
        for invalid in [0, 2] {
            let limits = LIMITS.replace(&format!("{field}: 1"), &format!("{field}: {invalid}"));
            assert!(platform_with_limits(Some(&limits)).validate().is_err());
        }
    }
    let zero_rate = LIMITS.replace('}', ", local_memory_service: {bytes_per_tick: 0}}");
    assert!(platform_with_limits(Some(&zero_rate)).validate().is_err());
    for invalid in [
        LIMITS.replace("lsu", "dma"),
        LIMITS.replace("shared_sram", "dedicated"),
        LIMITS.replace('}', ", surprise: 1}"),
    ] {
        assert!(
            yaml_serde::from_str::<
                gwr_models::processing_element::block_execution::BlockExecutionConfig,
            >(&invalid)
            .is_err()
        );
    }
    // Defaults are explicitly selected through YAML anchors.
    let source = format!(
        "memory_maps: [{{name: mm, devices: []}}]\ndefaults:\n  pe_config: &defaults\n    block_execution: {LIMITS}\nprocessing_elements:\n  - name: p0\n    memory_map: mm\n    config: *defaults\n  - name: p1\n    memory_map: mm\n    config:\n      block_execution: {{max_compute_tiles: 1, max_load_chunks: 1, max_store_tiles: 1, transfer_port: lsu, accumulator_storage: shared_sram, local_memory_service: {{bytes_per_tick: 7}}}}\n"
    );
    let config: PlatformConfig = yaml_serde::from_str(&source).unwrap();
    assert_eq!(
        config.processing_elements.unwrap()[1]
            .config
            .block_execution
            .as_ref()
            .unwrap()
            .local_memory_service
            .as_ref()
            .unwrap()
            .bytes_per_tick,
        7
    );
}
