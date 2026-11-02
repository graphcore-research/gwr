// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_engine::engine::Engine;
use gwr_models::processing_element::MachineOpCounts;
use gwr_platform::Platform;
use gwr_platform::connection_id::parse_connection_endpoint_id;
use gwr_platform::types::{
    CacheConfigSection, CacheSection, CoherencyManagerConfigSection, CoherencyManagerSection,
    ConnectSection, FabricKind, MemoryMapSection, MemoryStriperConfigSection, PlatformConfig,
};
use gwr_platform::yaml::platform_to_yaml_str;

const EXAMPLE: &str = include_str!("../examples/striped_memory.yaml");

fn example_config() -> PlatformConfig {
    yaml_serde::from_str(EXAMPLE).unwrap()
}

fn connection(first: &str, second: &str) -> ConnectSection {
    ConnectSection {
        connect: vec![first.into(), second.into()],
    }
}

fn add_outer_striper(config: &mut PlatformConfig) {
    config.memory_maps[0].devices = vec!["outer".into()];
    config.memory_maps.push(MemoryMapSection {
        name: "inner".into(),
        devices: vec!["s0".into()],
    });
    let stripers = config.memory_stripers.as_mut().unwrap();
    let mut outer = stripers[0].clone();
    outer.name = "outer".into();
    outer.memory_map = "inner".into();
    stripers.push(outer);
    let connections = config.connections.as_mut().unwrap();
    connections[0] = connection("pe.pe0", "striper.outer.dev");
    connections.push(connection("striper.outer.mem", "striper.s0.dev"));
}

fn cache(name: &str) -> CacheSection {
    CacheSection {
        name: name.into(),
        coherency_managers: None,
        config: CacheConfigSection {
            bw_bytes_per_tick: None,
            line_size_bytes: None,
            max_inflight_parents: None,
            max_inflight_fills: None,
            num_ways: None,
            num_sets: None,
            delay_ticks: None,
        },
    }
}

#[test]
fn stripe_size_bytes_must_be_explicit() {
    let source = EXAMPLE
        .lines()
        .filter(|line| !line.trim_start().starts_with("stripe_size_bytes:"))
        .collect::<Vec<_>>()
        .join("\n");
    let error = yaml_serde::from_str::<PlatformConfig>(&source).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("missing field `stripe_size_bytes`")
    );
}

#[test]
fn explicit_striper_endpoints_roundtrip() {
    for endpoint in ["striper.s.dev", "striper.s.mem"] {
        assert_eq!(
            parse_connection_endpoint_id(endpoint).unwrap().to_string(),
            endpoint
        );
    }
    for endpoint in ["striper.s", "striper.s.other"] {
        assert!(parse_connection_endpoint_id(endpoint).is_err());
    }
}

#[test]
fn builds_and_roundtrips_striped_memory_example() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_string(&engine, &clock, EXAMPLE).unwrap();
    assert_eq!(platform.num_memory_stripers(), 1);
    assert_eq!(platform.memory_striper("s0").unwrap().base_address(), 0);
    assert_eq!(
        platform.memory_striper("s0").unwrap().capacity_bytes(),
        1024
    );
    let config: PlatformConfig = yaml_serde::from_str(EXAMPLE).unwrap();
    let emitted = platform_to_yaml_str(&config).unwrap();
    assert!(emitted.contains("memory_stripers:"));
    let roundtrip: PlatformConfig = yaml_serde::from_str(&emitted).unwrap();
    roundtrip.validate().unwrap();
}

#[test]
fn independent_striper_admission_limits_roundtrip_and_run() {
    for (limits, parents, fragments) in [
        (
            "max_inflight_parents: 1\nmax_inflight_fragments: 4\n",
            Some(1),
            Some(4),
        ),
        (
            "max_inflight_parents: 4\nmax_inflight_fragments: 1\n",
            Some(4),
            Some(1),
        ),
        ("max_inflight_parents: 1\n", Some(1), None),
        ("max_inflight_fragments: 1\n", None, Some(1)),
        ("", None, None),
    ] {
        let source =
            format!("base_address: 0\ncapacity_bytes: 1024\nstripe_size_bytes: 64\n{limits}");
        let striper: MemoryStriperConfigSection = yaml_serde::from_str(&source).unwrap();
        assert_eq!(striper.max_inflight_parents, parents);
        assert_eq!(striper.max_inflight_fragments, fragments);
        let mut config = example_config();
        config.memory_stripers.as_mut().unwrap()[0].config = striper;
        let emitted = platform_to_yaml_str(&config).unwrap();
        assert_eq!(emitted.contains("max_inflight_parents:"), parents.is_some());
        assert_eq!(
            emitted.contains("max_inflight_fragments:"),
            fragments.is_some()
        );
        let roundtrip: PlatformConfig = yaml_serde::from_str(&emitted).unwrap();
        let striper = &roundtrip.memory_stripers.as_ref().unwrap()[0].config;
        assert_eq!(striper.max_inflight_parents, parents);
        assert_eq!(striper.max_inflight_fragments, fragments);
        roundtrip.validate().unwrap();

        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let platform = Platform::from_string(&engine, &clock, &emitted).unwrap();
        let dispatcher: std::rc::Rc<dyn gwr_models::processing_element::dispatch::Dispatch> =
            std::rc::Rc::new(OneTask::new());
        platform.attach_dispatcher(&dispatcher);
        engine.run().unwrap();
        assert_eq!(platform.pe("pe0").unwrap().bytes_consumed(), 160);
        assert_eq!(platform.pe("pe0").unwrap().bytes_produced(), 160);
        assert_eq!(platform.memory("m0").unwrap().bytes_read(), 96);
        assert_eq!(platform.memory("m1").unwrap().bytes_read(), 64);
        assert_eq!(platform.memory("m0").unwrap().bytes_written(), 64);
        assert_eq!(platform.memory("m1").unwrap().bytes_written(), 96);
    }
}

#[test]
fn rejects_invalid_striper_configuration_and_disconnected_ports() {
    let assert_invalid = |config: PlatformConfig, expected: &str| {
        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.to_lowercase().contains(&expected.to_lowercase()),
            "{error}"
        );
    };

    for (capacity_bytes, expected) in [(2048, "needs"), (0, "region")] {
        let mut config = example_config();
        config.memory_stripers.as_mut().unwrap()[0]
            .config
            .capacity_bytes = capacity_bytes;
        assert_invalid(config, expected);
    }

    let mut config = example_config();
    config.memory_stripers.as_mut().unwrap()[0]
        .config
        .stripe_size_bytes = 0;
    assert_invalid(config, "stripe");

    for (parents, fragments) in [(Some(0), Some(4)), (Some(4), Some(0))] {
        let mut config = example_config();
        let striper = &mut config.memory_stripers.as_mut().unwrap()[0].config;
        striper.max_inflight_parents = parents;
        striper.max_inflight_fragments = fragments;
        assert_invalid(config, "inflight");
    }

    let mut config = example_config();
    config.memory_stripers.as_mut().unwrap()[0].memory_map = "unknown".into();
    assert_invalid(config, "Unknown memory map");

    let mut config = example_config();
    config.connections.as_mut().unwrap()[1].connect[0] = "striper.missing.mem".into();
    assert_invalid(config, "No Memory Striper");

    let mut config = example_config();
    config.connections.as_mut().unwrap()[0] = connection("pe.pe0", "striper.s0.mem");
    assert_invalid(config, "striper");

    let mut config = example_config();
    let connections = config.connections.as_mut().unwrap();
    let before = connections.len();
    connections.retain(|connection| {
        !connection
            .connect
            .iter()
            .any(|endpoint| endpoint == "striper.s0.mem")
    });
    assert_eq!(
        connections.len(),
        before - 1,
        "disconnect exactly one striper memory port"
    );
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("not connected")
    );
}

#[test]
fn rejects_cyclic_backing_maps() {
    let mut config = example_config();
    config.memory_maps[1].devices = vec!["s0".into()];
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Cyclic")
    );
}

#[test]
fn pe_reads_and_writes_through_nested_stripers_and_both_fabric_kinds() {
    for fabric_kind in [FabricKind::Functional, FabricKind::Routed] {
        for nested in [false, true] {
            let mut config = example_config();
            config.fabrics.as_mut().unwrap()[0].kind = fabric_kind;
            if nested {
                add_outer_striper(&mut config);
            }
            let source = platform_to_yaml_str(&config).unwrap();
            let mut engine = Engine::default();
            let clock = engine.default_clock();
            let platform = Platform::from_string(&engine, &clock, &source).unwrap();
            let dispatcher: std::rc::Rc<dyn gwr_models::processing_element::dispatch::Dispatch> =
                std::rc::Rc::new(OneTask::new());
            platform.attach_dispatcher(&dispatcher);
            engine.run().unwrap();
            assert_eq!(platform.pe("pe0").unwrap().bytes_consumed(), 160);
            assert_eq!(platform.pe("pe0").unwrap().bytes_produced(), 160);
            assert_eq!(platform.memory("m0").unwrap().bytes_read(), 96);
            assert_eq!(platform.memory("m1").unwrap().bytes_read(), 64);
            assert_eq!(platform.memory("m0").unwrap().bytes_written(), 64);
            assert_eq!(platform.memory("m1").unwrap().bytes_written(), 96);
        }
    }
}

struct OneTask {
    active: std::cell::Cell<bool>,
    complete: std::cell::Cell<bool>,
    change: gwr_engine::events::repeated::Repeated<()>,
}

impl OneTask {
    fn new() -> Self {
        Self {
            active: std::cell::Cell::new(false),
            complete: std::cell::Cell::new(false),
            change: gwr_engine::events::repeated::Repeated::new(()),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl gwr_models::processing_element::dispatch::Dispatch for OneTask {
    fn task_by_id(
        &self,
        _index: usize,
    ) -> Result<gwr_models::processing_element::task::Task, gwr_engine::types::SimError> {
        use gwr_models::processing_element::operators::dtype::DataType;
        use gwr_models::processing_element::operators::{OperatorCustom, Tensor, TensorView};
        use gwr_models::processing_element::task::{ComputeOp, ComputeTaskConfig, Task};
        Ok(Task::ComputeTask {
            config: ComputeTaskConfig {
                id: "striped_rw".into(),
                op: ComputeOp::Custom(OperatorCustom {
                    name: None,
                    machine_ops: MachineOpCounts::default(),
                }),
                inputs: vec![Some(TensorView::new_full(Tensor::new(
                    &[160],
                    &DataType::Int8,
                    32,
                )?))],
                outputs: vec![Some(TensorView::new_full(Tensor::new(
                    &[160],
                    &DataType::Int8,
                    832,
                )?))],
            },
        })
    }
    fn set_task_active(&self, _index: usize) -> gwr_engine::types::SimResult {
        self.active.set(true);
        self.change.notify();
        Ok(())
    }
    fn set_task_completed(&self, _index: usize) -> gwr_engine::types::SimResult {
        self.complete.set(true);
        self.change.notify();
        Ok(())
    }
    fn ready_task_indices(
        &self,
        _name: &str,
    ) -> Result<(bool, Vec<usize>), gwr_engine::types::SimError> {
        Ok((
            self.complete.get(),
            if self.active.get() { vec![] } else { vec![0] },
        ))
    }
    async fn wait_for_change(&self) {
        use gwr_engine::traits::Event;
        self.change.listen().await;
    }
    fn total_tasks_for_pe(&self, _name: &str) -> usize {
        1
    }
}

#[test]
fn backing_devices_allow_byte_alignment_and_overlapping_logical_domains() {
    let mut config = example_config();
    let memories = config.memories.as_mut().unwrap();
    memories[0].base_address = 1;
    for memory in memories {
        memory.config.capacity_bytes = 513;
    }
    config.memory_stripers.as_mut().unwrap()[0]
        .config
        .stripe_size_bytes = 7;
    config.validate().unwrap();
}

#[test]
fn fabric_routes_stop_at_each_striper_side() {
    let mut config = example_config();
    let fabrics = config.fabrics.as_mut().unwrap();
    let mut front = fabrics[0].clone();
    front.name = "front".into();
    front.columns = 2;
    fabrics.push(front);
    let connections = config.connections.as_mut().unwrap();
    connections[0] = connection("pe.pe0", "fabric.front@(0,0)");
    connections.push(connection("striper.s0.dev", "fabric.front@(1,0)"));
    let source = platform_to_yaml_str(&config).unwrap();
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_string(&engine, &clock, &source).unwrap();
    assert_eq!(
        platform
            .fabric("front")
            .unwrap()
            .destination_port_map()
            .len(),
        2
    );
    assert_eq!(
        platform
            .fabric("backing")
            .unwrap()
            .destination_port_map()
            .len(),
        3
    );
    let dump = platform.fabric_port_map_description();
    assert!(dump.contains("(s0)"));
}

#[test]
fn rejects_two_striper_sides_in_the_same_fabric_domain() {
    let mut config = example_config();
    config.fabrics.as_mut().unwrap()[0]
        .config
        .fabric_ports_per_node = Some(2);
    let connections = config.connections.as_mut().unwrap();
    connections[0] = connection("pe.pe0", "fabric.backing@(0,0).1");
    connections.push(connection("striper.s0.dev", "fabric.backing@(1,0).1"));
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("same fabric domain")
    );
}

#[test]
fn rejects_unaligned_logical_apertures_used_by_pes() {
    let mut config = example_config();
    config.memory_stripers.as_mut().unwrap()[0]
        .config
        .base_address = 1;
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("aligned")
    );
}

#[test]
fn coherent_directory_can_use_a_striper_as_its_backing_device() {
    let mut config = example_config();
    let fabrics = config.fabrics.as_mut().unwrap();
    let mut front = fabrics[0].clone();
    front.name = "front".into();
    fabrics.push(front);
    let mut l1 = cache("l1");
    l1.coherency_managers = Some(vec!["cm".into()]);
    l1.config.num_ways = Some(1);
    l1.config.num_sets = Some(1);
    config.caches = Some(vec![l1]);
    config.coherency_managers = Some(vec![CoherencyManagerSection {
        name: "cm".into(),
        memory_map: "logical".into(),
        config: CoherencyManagerConfigSection {
            line_size_bytes: None,
            operations_per_tick: None,
        },
    }]);
    let connections = config.connections.as_mut().unwrap();
    connections[0] = connection("pe.pe0", "cache.l1.dev");
    connections.extend([
        connection("cache.l1.mem", "fabric.front@(0,0)"),
        connection("coherency_manager.cm", "fabric.front@(1,0)"),
        connection("striper.s0.dev", "fabric.front@(2,0)"),
    ]);
    let source = platform_to_yaml_str(&config).unwrap();
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_string(&engine, &clock, &source).unwrap();
    let dispatcher: std::rc::Rc<dyn gwr_models::processing_element::dispatch::Dispatch> =
        std::rc::Rc::new(OneTask::new());
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    assert_eq!(platform.pe("pe0").unwrap().bytes_consumed(), 160);
    assert_eq!(platform.pe("pe0").unwrap().bytes_produced(), 160);
    assert_eq!(platform.memory("m0").unwrap().bytes_read(), 96);
    assert_eq!(platform.memory("m1").unwrap().bytes_read(), 64);
    // Coherent writes remain dirty until eviction; a one-line cache writes
    // four of the five output lines back through the directory and striper.
    assert_eq!(platform.memory("m0").unwrap().bytes_written(), 64);
    assert_eq!(platform.memory("m1").unwrap().bytes_written(), 64);
    assert_eq!(
        platform
            .cache("l1")
            .unwrap()
            .traffic_snapshot()
            .logical_write_bytes,
        160
    );
}

#[test]
fn rejects_coherence_domains_that_alias_the_same_striper_backing() {
    let mut config = example_config();
    config.coherency_managers = Some(
        [("cm0", "logical"), ("cm1", "backing")]
            .into_iter()
            .map(|(name, memory_map)| CoherencyManagerSection {
                name: name.into(),
                memory_map: memory_map.into(),
                config: CoherencyManagerConfigSection {
                    line_size_bytes: None,
                    operations_per_tick: None,
                },
            })
            .collect(),
    );
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("multiple coherency managers")
    );
}

#[test]
fn backing_memory_reached_through_a_cache_still_requires_fetch_alignment() {
    let mut config = example_config();
    config.memory_maps[1].devices = vec!["m0".into()];
    let memories = config.memories.as_mut().unwrap();
    memories[0].base_address = 1;
    for memory in memories {
        memory.config.capacity_bytes = 1024;
    }
    config.caches = Some(vec![cache("lower")]);
    config.connections = Some(vec![
        connection("pe.pe0", "striper.s0.dev"),
        connection("striper.s0.mem", "cache.lower.dev"),
        connection("cache.lower.mem", "mem.m0"),
    ]);
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("aligned")
    );
}

#[test]
fn internal_striper_reached_through_a_cache_requires_fetch_alignment() {
    let mut config = example_config();
    add_outer_striper(&mut config);
    config.memory_stripers.as_mut().unwrap()[0]
        .config
        .base_address = 1;
    config.caches = Some(vec![cache("lower")]);
    let connections = config.connections.as_mut().unwrap();
    *connections.last_mut().unwrap() = connection("striper.outer.mem", "cache.lower.dev");
    connections.push(connection("cache.lower.mem", "striper.s0.dev"));
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("aligned")
    );
}
