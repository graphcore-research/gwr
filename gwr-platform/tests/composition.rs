// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::fs;

use gwr_platform::composition::load_source;

fn files(entries: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in entries {
        fs::write(dir.path().join(name), text).unwrap();
    }
    dir
}

#[test]
fn named_instances_reuse_definitions_without_losing_identity() {
    let dir = files(&[
        (
            "package.yaml",
            "name: chip\nlevel: package\nmemory_maps: []\n",
        ),
        (
            "board.yaml",
            "name: board\nlevel: board\ninstances:\n - {name: chip, file: package.yaml, count: 8}\n",
        ),
    ]);
    let source = load_source(&dir.path().join("board.yaml")).unwrap();
    assert_eq!(source.definitions.len(), 2);
    assert_eq!(source.files.len(), 2);
    let board = &source.definitions[&source.root];
    assert_eq!(board.instances[0].name, "chip");
    assert_eq!(board.instances[0].count, 8);
}

#[test]
fn cycles_and_unphysical_containment_are_rejected() {
    for (parent, child, content) in [
        ("rack", "package", ""),
        ("package", "board", ""),
        ("board", "component", ""),
        (
            "system",
            "rack",
            "memories: [{name: m, kind: hbm, base_address: 0, config: {capacity_bytes: 1024}}]",
        ),
    ] {
        let dir = files(&[
            (
                "parent.yaml",
                &format!(
                    "name: parent\nlevel: {parent}\ninstances: [{{name: child, file: child.yaml}}]\n{content}\n"
                ),
            ),
            ("child.yaml", &format!("name: child\nlevel: {child}\n")),
        ]);
        assert!(
            load_source(&dir.path().join("parent.yaml")).is_err(),
            "{parent} -> {child}"
        );
    }
    let dir = files(&[(
        "a.yaml",
        "level: board\ninstances: [{name: b, file: a.yaml}]\n",
    )]);
    assert!(
        load_source(&dir.path().join("a.yaml"))
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
}

#[test]
fn catalog_hashes_are_transitive_and_stable() {
    let dir = files(&[
        (
            "ops.yaml",
            "operation_classes: {add: {operations: [add], arity: 2}}\n",
        ),
        ("p.yaml", "includes: [ops.yaml]\nmemory_maps: []\n"),
    ]);
    let first = load_source(&dir.path().join("p.yaml")).unwrap();
    assert_eq!(first.files.len(), 2);
    fs::write(
        dir.path().join("ops.yaml"),
        "operation_classes: {mul: {operations: [mul], arity: 2}}\n",
    )
    .unwrap();
    let second = load_source(&dir.path().join("p.yaml")).unwrap();
    assert_ne!(first.files, second.files);
}

const CHIP: &str = r"
name: chip
level: package
defaults:
  pe_config: {sram_bytes: 4096, compute: scalar}
operation_classes: {add: {operations: [add], arity: 2}}
compute_profiles: {scalar: {engines: {alu: {capabilities: [{class: add, ops_per_tick: 4}]}}}}
memory_maps: [{name: local, devices: [ram]}]
processing_elements: [{name: cpu, memory_map: local, config: {}}]
memories: [{name: ram, kind: hbm, base_address: 0, config: {capacity_bytes: 1024}}]
fabrics: [{name: net, kind: functional, columns: 1, rows: 1, config: {fabric_ports_per_node: 2}}]
connections: [{connect: [pe.cpu, mem.ram]}]
ports: {uplink: 'fabric.net@(0,0)'}
";

#[test]
fn repeated_chips_get_distinct_addresses_defaults_and_parent_wiring() {
    let dir = files(&[
        ("chip.yaml", CHIP),
        (
            "board.yaml",
            "name: b\nlevel: board\ninstances: [{name: chip, file: chip.yaml, count: 2, address_base: 8KiB, address_stride: 4KiB}]\nconnections: [{connect: ['chip[0]/uplink', 'chip[1]/fabric.net@(0,0)']}]\n",
        ),
    ]);
    let source = load_source(&dir.path().join("board.yaml")).unwrap();
    let materialized = source.materialize(100).unwrap();
    let cfg: gwr_platform::types::PlatformConfig =
        yaml_serde::from_str(&materialized.resolved_yaml).unwrap();
    cfg.validate().unwrap();
    let memories = cfg.memories.unwrap();
    assert_eq!(memories[0].name, "chip[0]/ram");
    assert_eq!(memories[0].base_address, 8192);
    assert_eq!(memories[1].base_address, 12288);
    let pes = cfg.processing_elements.unwrap();
    assert_eq!(pes[1].memory_map, "chip[1]/local");
    assert_eq!(pes[1].config.sram_bytes, Some(4096));
    assert_eq!(
        cfg.connections.unwrap().last().unwrap().connect,
        vec!["fabric.chip[0]/net@(0,0)", "fabric.chip[1]/net@(0,0)"]
    );
}

#[test]
fn memory_repetition_requires_stride_and_checks_overflow_and_overlap() {
    for (base, stride) in [("0", "0"), ("18446744073709551600", "1024"), ("0", "512")] {
        let dir = files(&[
            ("chip.yaml", CHIP),
            (
                "board.yaml",
                &format!(
                    "level: board\ninstances: [{{name: chip, file: chip.yaml, count: 2, address_base: {base}, address_stride: {stride}}}]\n"
                ),
            ),
        ]);
        let source = load_source(&dir.path().join("board.yaml")).unwrap();
        assert!(
            source
                .materialize(100)
                .and_then(|m| {
                    let cfg: gwr_platform::types::PlatformConfig =
                        yaml_serde::from_str(&m.resolved_yaml).unwrap();
                    cfg.validate()
                })
                .is_err()
        );
    }
}

#[test]
fn aliases_cannot_reuse_internal_or_disabled_ports() {
    for endpoint in ["pe.cpu", "fabric.net@(0,0)"] {
        let chip = CHIP.replace(
            "uplink: 'fabric.net@(0,0)'",
            &format!("uplink: '{endpoint}'"),
        );
        let chip = if endpoint.starts_with("fabric") {
            format!("{chip}\n").replace("connections: [{connect: [pe.cpu, mem.ram]}]", "connections: [{connect: [pe.cpu, mem.ram]}, {connect: ['fabric.net@(0,0)', 'null']}]")
        } else {
            chip
        };
        let dir = files(&[
            ("chip.yaml", &chip),
            (
                "board.yaml",
                "level: board\ninstances: [{name: chip, file: chip.yaml, count: 2, address_stride: 4KiB}]\nconnections: [{connect: ['chip[0]/uplink', 'chip[1]/uplink']}]\n",
            ),
        ]);
        let source = load_source(&dir.path().join("board.yaml")).unwrap();
        assert!(source.materialize(100).is_err());
    }
}

#[test]
fn parent_binds_only_unresolved_child_maps() {
    let chip = CHIP.replace("memory_map: local", "memory_map: remote");
    let parent = "level: board\ninstances: [{name: chip, file: chip.yaml, memory_map_bindings: {remote: all}}]\nmemory_maps: [{name: all, devices: ['chip[0]/ram']}]\n";
    let dir = files(&[("chip.yaml", &chip), ("board.yaml", parent)]);
    let source = load_source(&dir.path().join("board.yaml")).unwrap();
    let m = source.materialize(100).unwrap();
    let cfg: gwr_platform::types::PlatformConfig = yaml_serde::from_str(&m.resolved_yaml).unwrap();
    assert_eq!(cfg.processing_elements.unwrap()[0].memory_map, "all");
    fs::write(
        dir.path().join("board.yaml"),
        parent.replace("remote: all", "local: all"),
    )
    .unwrap();
    assert!(
        load_source(&dir.path().join("board.yaml"))
            .unwrap()
            .materialize(100)
            .is_err()
    );
}

#[test]
fn huge_instance_counts_stay_compact_and_materialization_is_bounded() {
    let dir = files(&[
        ("chip.yaml", "level: package\n"),
        (
            "board.yaml",
            "level: board\ninstances: [{name: chip, file: chip.yaml, count: 1000000000}]\n",
        ),
    ]);
    let source = load_source(&dir.path().join("board.yaml")).unwrap();
    assert_eq!(source.definitions.len(), 2);
    assert!(source.materialize(100).is_err());
}

#[test]
fn bounded_connection_loops_build_a_chain_with_index_arithmetic() {
    let dir = files(&[
        (
            "chip.yaml",
            "level: package\nfabrics: [{name: net, kind: functional, columns: 1, rows: 1, config: {fabric_ports_per_node: 2}}]\nports: {left: 'fabric.net@(0,0)', right: 'fabric.net@(0,0).1'}\n",
        ),
        (
            "board.yaml",
            "level: board\ninstances: [{name: chip, file: chip.yaml, count: 4}]\nrepeat_connections:\n - for: {i: [0, 3]}\n   connect: ['chip[${i}]/right', 'chip[${i + 1}]/left']\nconnections:\n - connect: ['chip[0]/left', 'null']\n - connect: ['chip[3]/right', 'null']\n",
        ),
    ]);
    let source = load_source(&dir.path().join("board.yaml")).unwrap();
    let m = source.materialize(100).unwrap();
    let cfg: gwr_platform::types::PlatformConfig = yaml_serde::from_str(&m.resolved_yaml).unwrap();
    assert_eq!(cfg.connections.as_ref().unwrap().len(), 5);
    let mut engine = gwr_engine::engine::Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    gwr_platform::Platform::from_file(&engine, &clock, &dir.path().join("board.yaml")).unwrap();
    engine.run().unwrap();
}

#[test]
fn loops_reject_unknown_variables_unbounded_ranges_and_unsafe_expressions() {
    for (range, expression) in [
        ("[0, 1]", "missing"),
        ("[2, 1]", "i"),
        ("[0, 1000000000]", "i"),
        ("[0, 1]", "i / 2"),
        ("[0, 1]", "i - 1"),
    ] {
        let dir = files(&[(
            "p.yaml",
            &format!(
                "level: package\nrepeat_connections:\n - for: {{i: {range}}}\n   connect: ['fabric.f@(${{{expression}}},0)', 'null']\n"
            ),
        )]);
        assert!(
            load_source(&dir.path().join("p.yaml"))
                .and_then(|s| s.materialize(100))
                .is_err(),
            "{range}: {expression}"
        );
    }
}

#[test]
fn composed_maps_require_a_physical_request_path() {
    let chip = CHIP.replace(
        "connections: [{connect: [pe.cpu, mem.ram]}]",
        "connections: []",
    );
    let dir = files(&[("chip.yaml", &chip)]);
    assert!(
        load_source(&dir.path().join("chip.yaml"))
            .unwrap()
            .materialize(100)
            .is_err()
    );
}

#[test]
fn scoped_native_snapshot_can_be_loaded_again() {
    let dir = files(&[
        ("chip.yaml", CHIP),
        (
            "b.yaml",
            "level: board\ninstances: [{name: chip, file: chip.yaml, count: 2, address_stride: 4KiB}]\n",
        ),
    ]);
    let m = load_source(&dir.path().join("b.yaml"))
        .unwrap()
        .materialize(100)
        .unwrap();
    fs::write(dir.path().join("flat.yaml"), &m.resolved_yaml).unwrap();
    let reloaded = load_source(&dir.path().join("flat.yaml"))
        .unwrap()
        .materialize(100)
        .unwrap();
    let cfg: gwr_platform::types::PlatformConfig =
        yaml_serde::from_str(&reloaded.resolved_yaml).unwrap();
    cfg.validate().unwrap();
}

#[test]
fn hierarchical_example_builds_and_dma_maps_stay_board_scoped() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/composition/system.yaml");
    let source = load_source(&path).unwrap();
    let m = source.materialize(1000).unwrap();
    let cfg: gwr_platform::types::PlatformConfig = yaml_serde::from_str(&m.resolved_yaml).unwrap();
    assert_eq!(cfg.processing_elements.as_ref().unwrap().len(), 8);
    assert_eq!(
        cfg.memories.as_ref().unwrap().last().unwrap().base_address,
        28672
    );
    let pe = &cfg.processing_elements.as_ref().unwrap()[0];
    assert_eq!(pe.memory_map, "rack[0]/board[0]/chip[0]/local");
    assert_eq!(
        pe.dma_memory_map.as_deref(),
        Some("rack[0]/board[0]/board_memory")
    );
    assert_eq!(
        cfg.memory_maps
            .iter()
            .find(|m| m.name == pe.memory_map)
            .unwrap()
            .devices
            .len(),
        1
    );
    assert_eq!(
        cfg.memory_maps
            .iter()
            .find(|m| Some(&m.name) == pe.dma_memory_map.as_ref())
            .unwrap()
            .devices
            .len(),
        2
    );
    let mut engine = gwr_engine::engine::Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    gwr_platform::Platform::from_file(&engine, &clock, &path).unwrap();
}

#[test]
fn materialization_limit_counts_fabric_tiles() {
    let dir = files(&[(
        "p.yaml",
        "level: package\nfabrics: [{name: huge, kind: functional, columns: 1000000, rows: 1000000, config: {}}]\n",
    )]);
    assert!(
        load_source(&dir.path().join("p.yaml"))
            .unwrap()
            .materialize(100)
            .is_err()
    );
}

#[test]
fn ddr_is_an_external_memory_package_in_strict_hierarchies() {
    let dir = files(&[("p.yaml", &CHIP.replace("kind: hbm", "kind: ddr"))]);
    assert!(load_source(&dir.path().join("p.yaml")).is_err());
    fs::write(dir.path().join("p.yaml"),"level: package\nmemories: [{name: d,kind: ddr,base_address: 0,config: {capacity_bytes: 1024}}]\n").unwrap();
    assert!(
        load_source(&dir.path().join("p.yaml"))
            .unwrap()
            .materialize(100)
            .is_ok()
    );
}

#[test]
fn composed_memory_can_end_at_the_last_u64_address() {
    let dir = files(&[(
        "p.yaml",
        "level: package\nmemories: [{name: last, kind: hbm, base_address: 18446744073709551615, config: {capacity_bytes: 1}}]\n",
    )]);
    let m = load_source(&dir.path().join("p.yaml"))
        .unwrap()
        .materialize(100)
        .unwrap();
    assert_eq!(m.memory_windows[0].global_base, "18446744073709551615");
}

#[test]
fn empty_inner_loop_short_circuits_even_a_huge_outer_range() {
    let dir = files(&[(
        "p.yaml",
        "level: package\nrepeat_connections:\n - for: {a: [0, 4294967295], z: [0, 0]}\n   connect: ['fabric.f@(${a},0)', 'null']\n",
    )]);
    let source = load_source(&dir.path().join("p.yaml")).unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || send.send(source.materialize(100)).unwrap());
    recv.recv_timeout(std::time::Duration::from_secs(2))
        .expect("empty loop must do bounded work")
        .unwrap();
}
