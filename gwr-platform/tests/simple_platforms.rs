// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::Event;
use gwr_engine::types::SimError;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::task::{ProgramTaskConfig, Task};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_platform::Platform;

macro_rules! pe_mem_config {
    ($num_active_requests:expr) => {
        format!(
            "
memory_maps:
  - name: mm0
    devices:
      - hbm0

operation_classes:
  add:
    operations: [add]
    arity: 2

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
      num_active_requests: {}
      lsu_access_bytes: 32
      compute:
        engines:
          vector:
            capabilities:
              - {{ class: add, ops_per_tick: 256 }}

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    config:
      capacity_bytes: 16GiB
      delay_ticks: 10

connections:
  - connect:
    - pe.pe0
    - mem.hbm0
",
            $num_active_requests
        )
        .as_str()
    };
}

#[test]
fn simple_pe_mem_one_request() {
    let (_, clock) = run_pe_mem(1);

    // There are two compute tasks, each issuing two 32-byte reads and one
    // 32-byte write. Each memory access has a 10ns delay.
    assert_eq!(clock.time_now_ns(), 60.0);
}

#[test]
fn simple_pe_mem_two_requests() {
    let (_, clock) = run_pe_mem(2);

    // Two compute tasks can overlap their LSU requests when the LSU has two
    // outstanding request slots.
    assert_eq!(clock.time_now_ns(), 32.0);
}

fn run_pe_mem(num_active_requests: usize) -> (Engine, Clock) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform =
        Platform::from_string(&engine, &clock, pe_mem_config!(num_active_requests)).unwrap();

    assert_eq!(platform.num_pes(), 1);
    assert_eq!(platform.num_memories(), 1);
    assert_eq!(platform.num_fabrics(), 0);
    assert_eq!(platform.num_caches(), 0);

    let dispatcher = build_dispatcher();
    platform.attach_dispatcher(&dispatcher);

    run_simulation!(engine);
    (engine, clock)
}

#[test]
fn strided_ranges_share_active_requests() {
    let run = |strided| {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let platform = strided_platform(&engine, &clock, 16);
        platform.attach_dispatcher(&access_dispatcher(strided));

        engine.run().unwrap();
        clock.time_now_ns()
    };

    assert_eq!(run(true), run(false));
}

#[test]
fn oversized_explicit_tile_checks_the_complete_working_set() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = strided_platform(&engine, &clock, 1);
    platform.attach_dispatcher(&access_dispatcher(true));

    let error = engine.run().unwrap_err();
    assert!(error.to_string().contains("tile requires 16 SRAM bytes"));
    assert_eq!(clock.time_now_ns(), 0.0);
}

fn access_dispatcher(strided: bool) -> Rc<dyn Dispatch> {
    let view = if strided {
        let tensor = Tensor::new(&[8, 2], &DataType::Int8, 0x1_0000_0000).unwrap();
        TensorView::new(tensor, &[8, 1], &[0, 0]).unwrap()
    } else {
        TensorView::new_full(Tensor::new(&[8], &DataType::Int8, 0x1_0000_0000).unwrap())
    };
    let output = TensorView::new_full(Tensor::new(&[8], &DataType::Int8, 0x1_0000_0100).unwrap());
    let indices = if strided { "[i, 0]" } else { "[i]" };
    let program: ProgramSpec = yaml_serde::from_str(&format!(
        "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: {{ read: {{ tensor: A, indices: {indices} }} }}\n"
    )).unwrap();
    let task = Task::ProgramTask {
        config: Box::new(ProgramTaskConfig {
            pipeline: None,
            id: "read".to_string(),
            program,
            tiling: None,
            inputs: vec![view],
            outputs: vec![output],
        }),
    };

    Rc::new(TestDispatcher::new(
        HashMap::from([(0, task)]),
        HashMap::from([("pe0".to_string(), VecDeque::from([0]))]),
    ))
}

fn strided_platform(engine: &Engine, clock: &Clock, sram_bytes: usize) -> Platform {
    Platform::from_string(
        engine,
        clock,
        &format!(
            "
memory_maps:
  - name: mm0
    devices:
      - hbm0

operation_classes:
  add:
    operations: [add]
    arity: 2

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
      num_active_requests: 8
      lsu_access_bytes: 1
      sram_bytes: {sram_bytes}
      compute:
        engines:
          vector:
            capabilities:
              - {{ class: add, ops_per_tick: 256 }}

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    config:
      capacity_bytes: 16GiB
      delay_ticks: 10

connections:
  - connect:
    - pe.pe0
    - mem.hbm0
"
        ),
    )
    .unwrap()
}

#[test]
fn simple_pe_fabric_mem_routes_by_device_id() {
    let (_, clock) = run_pe_fabric_mem(1);

    assert!(clock.time_now_ns() > 80.0);
}

fn run_pe_fabric_mem(num_active_requests: usize) -> (Engine, Clock) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Platform::from_string(
        &engine,
        &clock,
        format!(
            "
memory_maps:
  - name: mm0
    devices:
      - hbm0

operation_classes:
  add:
    operations: [add]
    arity: 2

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
      num_active_requests: {num_active_requests}
      lsu_access_bytes: 32
      compute:
        engines:
          vector:
            capabilities:
              - {{ class: add, ops_per_tick: 256 }}

fabrics:
  - name: fabric0
    kind: functional
    columns: 2
    rows: 1
    config:
      fabric_ports_per_node: 1
      ticks_per_hop: 1
      ticks_overhead: 1
      port_bits_per_tick: 4096

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    config:
      capacity_bytes: 16GiB
      delay_ticks: 10

connections:
  - connect:
    - pe.pe0
    - fabric.fabric0@(0,0)
  - connect:
    - mem.hbm0
    - fabric.fabric0@(1,0)
"
        )
        .as_str(),
    )
    .unwrap();

    assert_eq!(platform.num_pes(), 1);
    assert_eq!(platform.num_memories(), 1);
    assert_eq!(platform.num_fabrics(), 1);
    assert_eq!(platform.num_caches(), 0);
    let port_map_dump = platform.fabric_port_map_description();
    assert!(port_map_dump.contains("Fabric fabric0 port_selection: destination-address-hash"));
    assert!(port_map_dump.contains("device 0 (pe0) -> 0=fabric.fabric0@(0,0) (0,0).0"));
    assert!(port_map_dump.contains("device 1 (hbm0) -> 1=fabric.fabric0@(1,0) (1,0).0"));

    let dispatcher = build_dispatcher();
    platform.attach_dispatcher(&dispatcher);

    run_simulation!(engine);
    (engine, clock)
}

#[test]
fn simple_pe_cache_mem() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: mm0
    devices:
      - hbm0

operation_classes:
  add:
    operations: [add]
    arity: 2

processing_elements:
  - name: pe0
    memory_map: mm0
    config:
      num_active_requests: 1
      lsu_access_bytes: 32
      compute:
        engines:
          vector:
            capabilities:
              - { class: add, ops_per_tick: 256 }

caches:
  - name: c0
    config:
      delay_ticks: 5

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
    config:
      capacity_bytes: 16GiB
      delay_ticks: 20

connections:
  - connect:
    - pe.pe0
    - cache.c0.dev
  - connect:
    - cache.c0.mem
    - mem.hbm0
",
    )
    .unwrap();

    assert_eq!(platform.num_pes(), 1);
    assert_eq!(platform.num_memories(), 1);
    assert_eq!(platform.num_fabrics(), 0);
    assert_eq!(platform.num_caches(), 1);

    let dispatcher = build_dispatcher();
    platform.attach_dispatcher(&dispatcher);

    run_simulation!(engine);

    // Expect cache hits for repeated tensor reads plus memory traffic for
    // first reads and writes. Each32B read response with8B header occupies
    // two32B/tick service ticks. Fill ingress and shared client response
    // service now finish before data becomes usable.
    let cache_activity = platform.cache("c0").unwrap().traffic_snapshot();
    assert_eq!(
        (cache_activity.request_hits, cache_activity.request_misses),
        (2, 2)
    );
    assert_eq!(
        (
            cache_activity.downstream_read_bytes,
            cache_activity.downstream_write_bytes
        ),
        (64, 64)
    );
    assert_eq!(clock.time_now_ns(), 154.0);
}

fn build_dispatcher() -> Rc<dyn Dispatch> {
    let input_a = Tensor::new(&[8], &DataType::Fp32, 0x1_0000_0000).unwrap();
    let input_b = Tensor::new(&[8], &DataType::Fp32, 0x1_0000_0100).unwrap();
    let output = Tensor::new(&[8], &DataType::Fp32, 0x1_0000_0200).unwrap();
    let program: ProgramSpec = yaml_serde::from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [d0]
    expr: { add: [{ read: { tensor: A, indices: [d0] } }, { read: { tensor: B, indices: [d0] } }] }
",
    )
    .unwrap();
    let task = |id: &str| Task::ProgramTask {
        config: Box::new(ProgramTaskConfig {
            pipeline: None,
            id: id.to_string(),
            program: program.clone(),
            tiling: None,
            inputs: vec![
                TensorView::new_full(input_a.clone()),
                TensorView::new_full(input_b.clone()),
            ],
            outputs: vec![TensorView::new_full(output.clone())],
        }),
    };

    Rc::new(TestDispatcher::new(
        HashMap::from([(0, task("task0")), (1, task("task1"))]),
        HashMap::from([("pe0".to_string(), VecDeque::from([0, 1]))]),
    ))
}

/// A struct that implements `Dispatch`
///
/// It assumes all tasks are ready, but just returns the next task for the PE
struct TestDispatcher {
    tasks: RefCell<HashMap<usize, Task>>,
    tasks_by_pe: RefCell<HashMap<String, VecDeque<usize>>>,
    completed_tasks: RefCell<HashSet<usize>>,
    started_tasks: RefCell<HashSet<usize>>,
    change: Repeated<()>,
}

impl TestDispatcher {
    fn new(tasks: HashMap<usize, Task>, tasks_by_pe: HashMap<String, VecDeque<usize>>) -> Self {
        Self {
            tasks: RefCell::new(tasks),
            tasks_by_pe: RefCell::new(tasks_by_pe),
            completed_tasks: RefCell::new(HashSet::new()),
            started_tasks: RefCell::new(HashSet::new()),
            change: Repeated::new(()),
        }
    }
}

#[async_trait(?Send)]
impl Dispatch for TestDispatcher {
    fn ready_task_indices(&self, pe_name: &str) -> Result<(bool, Vec<usize>), SimError> {
        let mut handle = self.tasks_by_pe.borrow_mut();
        match handle.get_mut(pe_name) {
            None => Ok((true, Vec::new())),
            Some(v) => loop {
                let idx = match v.front() {
                    None => return Ok((true, Vec::new())),
                    Some(i) => i,
                };
                if self.completed_tasks.borrow().contains(idx)
                    || self.started_tasks.borrow().contains(idx)
                {
                    v.pop_front();
                } else {
                    return Ok((false, vec![*idx]));
                }
            },
        }
    }

    fn set_task_active(&self, task_idx: usize) -> gwr_engine::types::SimResult {
        let mut handle = self.started_tasks.borrow_mut();
        handle.insert(task_idx);
        self.change.notify();
        Ok(())
    }

    fn set_task_completed(&self, task_idx: usize) -> gwr_engine::types::SimResult {
        let mut handle = self.completed_tasks.borrow_mut();
        handle.insert(task_idx);
        self.change.notify();
        Ok(())
    }

    fn task_by_id(&self, task_idx: usize) -> Result<Task, SimError> {
        let handle = self.tasks.borrow_mut();
        let task = handle
            .get(&task_idx)
            .ok_or(SimError(format!("Invalid task_idx '{task_idx}'")))?;
        Ok(task.clone())
    }

    fn total_tasks_for_pe(&self, pe_name: &str) -> usize {
        let mut handle = self.tasks_by_pe.borrow_mut();
        match handle.get_mut(pe_name) {
            None => 0,
            Some(v) => v.len(),
        }
    }

    async fn wait_for_change(&self) {
        self.change.listen().await;
    }
}

#[test]
fn explicit_cache_device_fabric_port_routes_real_requests_and_responses() {
    let mut config: serde_json::Value = yaml_serde::from_str(pe_mem_config!(1)).unwrap();
    config["caches"] = serde_json::json!([{ "name":"shared", "config":{
        "line_size_bytes":128,"num_ways":1,"num_sets":16,
        "bw_bytes_per_tick":32,"delay_ticks":2}}]);
    config["fabrics"] = serde_json::json!([{ "name":"local","kind":"functional","columns":2,"rows":1,
        "config":{"fabric_ports_per_node":1,"ticks_per_hop":1,"ticks_overhead":0,
        "rx_buffer_bytes":4096,"tx_buffer_bytes":4096,"port_bits_per_tick":512}}]);
    config["connections"] = serde_json::json!([
        {"connect":["pe.pe0","fabric.local@(0,0)"]},
        {"connect":["fabric.local@(1,0)","cache.shared.dev"]},
        {"connect":["cache.shared.mem","mem.hbm0"]}]);
    for reverse in [false, true] {
        let mut connected = config.clone();
        if reverse {
            connected["connections"][1]["connect"] =
                serde_json::json!(["cache.shared.dev", "fabric.local@(1,0)"]);
        }
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let platform =
            Platform::from_string(&engine, &clock, &yaml_serde::to_string(&connected).unwrap())
                .unwrap();
        platform.attach_dispatcher(&build_dispatcher());
        engine.run().unwrap();
        // Each initial read fetches a complete 128-byte line.
        assert_eq!(platform.memory("hbm0").unwrap().bytes_read(), 256);
        assert_eq!(platform.memory("hbm0").unwrap().bytes_written(), 64);
        let cache = platform.cache("shared").unwrap();
        assert_eq!(cache.num_misses(), 2);
        assert_eq!(cache.num_hits(), 2);
        assert_eq!(cache.payload_bytes_read(), 128);
        assert_eq!(cache.payload_bytes_written(), 64);
    }
    let mut invalid_port = config.clone();
    invalid_port["connections"][1]["connect"][1] = serde_json::json!("cache.shared.invalid");
    let mut invalid_memory = config.clone();
    invalid_memory["connections"][2]["connect"][0] = serde_json::json!("cache.shared.dev");
    let mut duplicate = config.clone();
    duplicate["connections"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"connect":["fabric.local@(1,0)","cache.shared.dev"]}));
    for rejected in [invalid_port, invalid_memory, duplicate] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        assert!(
            Platform::from_string(&engine, &clock, &yaml_serde::to_string(&rejected).unwrap(),)
                .is_err()
        );
    }
}

// Preserve main's fixed-size LSU regression coverage with program tasks.
#[test]
fn lsu_services_full_access_chunks_for_partial_reads_and_writes() {
    for (address, bytes, physical_bytes, write_address) in [
        (0x1_0000_0000, 1, 32, 0x1_0000_0100),
        (0x1_0000_0000, 33, 64, 0x1_0000_0100),
        (0x4_ffff_ffff, 1, 32, 0x1_0000_0100),
        (0x1_0000_0000, 1, 32, 0x4_ffff_ffff),
        (0x4_ffff_ffdf, 33, 64, 0x1_0000_0100),
        (0x1_0000_001f, 2, 64, 0x1_0000_011f),
        (0x1_0000_0001, 32, 64, 0x1_0000_0101),
    ] {
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let platform = Platform::from_string(&engine, &clock, pe_mem_config!(1)).unwrap();
        let program = yaml_serde::from_str(
            "inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [i]\n   expr: {reduce: {op: sum, over: [k], value: {read: {tensor: A, indices: [i, k]}}}}\n",
        ).unwrap();
        let task = Task::ProgramTask {
            config: Box::new(ProgramTaskConfig {
                pipeline: None,
                id: "partial_transfer".into(),
                program,
                inputs: vec![TensorView::new_full(
                    Tensor::new(&[1, bytes], &DataType::Int8, address).unwrap(),
                )],
                outputs: vec![TensorView::new_full(
                    Tensor::new(&[1], &DataType::Int8, write_address).unwrap(),
                )],
                tiling: None,
            }),
        };
        let dispatcher: Rc<dyn Dispatch> = Rc::new(TestDispatcher::new(
            HashMap::from([(0, task)]),
            HashMap::from([("pe0".into(), VecDeque::from([0]))]),
        ));
        platform.attach_dispatcher(&dispatcher);
        engine.run().unwrap();
        let memory = platform.memory("hbm0").unwrap();
        assert_eq!(memory.bytes_read(), physical_bytes);
        assert_eq!(memory.bytes_written(), 32);
        assert_eq!(
            platform.pe("pe0").unwrap().memory_traffic_bytes(),
            (bytes, 1)
        );
    }
}

#[test]
fn coherent_examples_run_with_one_parent_and_one_child_slot() {
    for source in [
        include_str!("../examples/simple_pe_cache_coherent_mem.yaml"),
        include_str!("../examples/coherent_2mem_1manager.yaml"),
        include_str!("../examples/coherent_2mem_2managers.yaml"),
    ] {
        for routed in [false, true] {
            // Restrict each example to one tracked parent and one active fill.
            let mut engine = start_test(file!());
            let clock = engine.default_clock();
            let source = source.replace("      bw_bytes_per_tick: 32", "      bw_bytes_per_tick: 32\n      max_inflight_parents: 1\n      max_inflight_fills: 1");

            // Exercise the same workload with both fabric implementations.
            let source = if routed {
                source.replace("kind: functional", "kind: routed")
            } else {
                source
            };

            // Build the platform, attach work, and run it to completion.
            let platform = Platform::from_string(&engine, &clock, &source).unwrap();
            platform.attach_dispatcher(&build_dispatcher());
            run_simulation!(engine);

            // Check that coherence ran without exceeding the fill limit.
            assert!(
                platform
                    .coherency_manager("cm0")
                    .unwrap()
                    .received_coherence_message_count()
                    > 0
            );
            assert!(
                platform
                    .cache("l1_0")
                    .unwrap()
                    .inflight_fills_snapshot()
                    .peak
                    <= 1
            );
        }
    }
}
