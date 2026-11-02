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
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::task::{ProgramTaskConfig, Task};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_platform::Platform;
use yaml_serde::from_str;

const LSU_ACCESS_BYTES: usize = 32;
const MEMORY_BASE_ADDRESS: u64 = 0x1_0000_0000;
const MEMORY_CAPACITY_BYTES: usize = 16 * 1024 * 1024 * 1024;

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

    fn set_task_active(&self, task_idx: usize) -> SimResult {
        let mut handle = self.started_tasks.borrow_mut();
        handle.insert(task_idx);
        self.change.notify();
        Ok(())
    }

    fn set_task_completed(&self, task_idx: usize) -> SimResult {
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

fn two_add_tasks_dispatcher() -> Rc<dyn Dispatch> {
    let input_a = Tensor::new(&[8], &DataType::Fp32, MEMORY_BASE_ADDRESS).unwrap();
    let input_b = Tensor::new(&[8], &DataType::Fp32, MEMORY_BASE_ADDRESS + 0x100).unwrap();
    let output = Tensor::new(&[8], &DataType::Fp32, MEMORY_BASE_ADDRESS + 0x200).unwrap();
    let program: ProgramSpec = from_str(
        r"
inputs: [A, B]
outputs: [Y]
assignments:
  - output: Y
    axes: [d0]
    expr:
      add:
        - read: { tensor: A, indices: [d0] }
        - read: { tensor: B, indices: [d0] }
",
    )
    .unwrap();
    let task = |id: &str| Task::ProgramTask {
        config: Box::new(ProgramTaskConfig {
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
      lsu_access_bytes: {LSU_ACCESS_BYTES}
      compute:
        execution_units:
          vector:
            capabilities:
              - {{ class: add, ops_per_tick: 256 }}

memories:
  - name: hbm0
    kind: hbm
    base_address: {MEMORY_BASE_ADDRESS}
    config:
      capacity_bytes: {MEMORY_CAPACITY_BYTES}
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

fn run_compute_with_direct_memory(num_active_requests: usize) -> (Engine, Clock) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform =
        Platform::from_string(&engine, &clock, pe_mem_config!(num_active_requests)).unwrap();

    assert_eq!(platform.num_pes(), 1);
    assert_eq!(platform.num_memories(), 1);
    assert_eq!(platform.num_fabrics(), 0);
    assert_eq!(platform.num_caches(), 0);

    let dispatcher = two_add_tasks_dispatcher();
    platform.attach_dispatcher(&dispatcher);

    run_simulation!(engine);
    (engine, clock)
}

fn run_compute_with_fabric_memory(num_active_requests: usize) -> (Engine, Clock) {
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
      lsu_access_bytes: {LSU_ACCESS_BYTES}
      compute:
        execution_units:
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
    base_address: {MEMORY_BASE_ADDRESS}
    config:
      capacity_bytes: {MEMORY_CAPACITY_BYTES}
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

    let dispatcher = two_add_tasks_dispatcher();
    platform.attach_dispatcher(&dispatcher);

    run_simulation!(engine);
    (engine, clock)
}

fn strided_or_contiguous_copy_dispatcher(strided: bool) -> Rc<dyn Dispatch> {
    let view = if strided {
        let tensor = Tensor::new(&[8, 2], &DataType::Int8, MEMORY_BASE_ADDRESS).unwrap();
        TensorView::new(tensor, &[8, 1], &[0, 0]).unwrap()
    } else {
        TensorView::new_full(Tensor::new(&[8], &DataType::Int8, MEMORY_BASE_ADDRESS).unwrap())
    };
    let output = TensorView::new_full(
        Tensor::new(&[8], &DataType::Int8, MEMORY_BASE_ADDRESS + 0x100).unwrap(),
    );
    let indices = if strided { "[i, 0]" } else { "[i]" };
    let program: ProgramSpec = from_str(&format!(
        r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      read: {{ tensor: A, indices: {indices} }}
"
    ))
    .unwrap();
    let task = Task::ProgramTask {
        config: Box::new(ProgramTaskConfig {
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

fn copy_platform_with_sram(engine: &Engine, clock: &Clock, sram_bytes: usize) -> Platform {
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
        execution_units:
          vector:
            capabilities:
              - {{ class: add, ops_per_tick: 256 }}

memories:
  - name: hbm0
    kind: hbm
    base_address: {MEMORY_BASE_ADDRESS}
    config:
      capacity_bytes: {MEMORY_CAPACITY_BYTES}
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

fn copy_completion_time(strided: bool) -> f64 {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = copy_platform_with_sram(&engine, &clock, 16);
    platform.attach_dispatcher(&strided_or_contiguous_copy_dispatcher(strided));

    engine.run().unwrap();
    clock.time_now_ns()
}

fn run_partial_transfer(address: u64, bytes: usize) -> Platform {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_string(&engine, &clock, pe_mem_config!(1)).unwrap();
    let program = from_str(
        r"
inputs: [A]
outputs: [Y]
assignments:
  - output: Y
    axes: [i]
    expr:
      read: { tensor: A, indices: [i] }
",
    )
    .unwrap();
    let task = Task::ProgramTask {
        config: Box::new(ProgramTaskConfig {
            id: "partial_transfer".into(),
            program,
            inputs: vec![TensorView::new_full(
                Tensor::new(&[bytes], &DataType::Int8, address).unwrap(),
            )],
            outputs: vec![TensorView::new_full(
                Tensor::new(&[bytes], &DataType::Int8, MEMORY_BASE_ADDRESS + 0x100).unwrap(),
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
    platform
}

#[test]
fn insufficient_sram_rejects_the_complete_working_set_before_execution() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = copy_platform_with_sram(&engine, &clock, 1);
    platform.attach_dispatcher(&strided_or_contiguous_copy_dispatcher(true));

    let error = engine.run().unwrap_err();
    assert!(error.to_string().contains("tile requires 16 SRAM bytes"));
    assert_eq!(clock.time_now_ns(), 0.0);
}

#[test]
fn one_lsu_request_slot_serializes_memory_accesses_from_two_compute_tasks() {
    let (_, clock) = run_compute_with_direct_memory(1);

    // There are two compute tasks, each issuing two 32-byte reads and one
    // 32-byte write. Each memory access has a 10ns delay.
    assert_eq!(clock.time_now_ns(), 60.0);
}

#[test]
fn two_lsu_request_slots_allow_compute_task_memory_accesses_to_overlap() {
    let (_, clock) = run_compute_with_direct_memory(2);

    // Two compute tasks can overlap their LSU requests when the LSU has two
    // outstanding request slots.
    assert_eq!(clock.time_now_ns(), 32.0);
}

#[test]
fn strided_and_contiguous_reads_have_equal_completion_times() {
    assert_eq!(copy_completion_time(true), copy_completion_time(false));
}

// Preserve main's fixed-size LSU regression coverage with program tasks.
#[test]
fn lsu_services_full_access_chunks_for_partial_reads_and_writes() {
    let base_address = MEMORY_BASE_ADDRESS;
    let access_bytes = LSU_ACCESS_BYTES;
    for (address, bytes, physical_bytes) in [
        (base_address, 1, access_bytes),
        (base_address, access_bytes + 1, 2 * access_bytes),
        (base_address + access_bytes as u64 - 1, 2, 2 * access_bytes),
        (base_address + 1, access_bytes, 2 * access_bytes),
        (
            MEMORY_BASE_ADDRESS + MEMORY_CAPACITY_BYTES as u64 - 1,
            1,
            access_bytes,
        ),
    ] {
        let platform = run_partial_transfer(address, bytes);
        let memory = platform.memory("hbm0").unwrap();
        assert_eq!(memory.bytes_read(), physical_bytes);
        assert_eq!(
            memory.bytes_written(),
            bytes.div_ceil(LSU_ACCESS_BYTES) * LSU_ACCESS_BYTES
        );
        assert_eq!(
            platform.pe("pe0").unwrap().memory_traffic_bytes(),
            (bytes, bytes)
        );
    }
}

#[test]
fn compute_tasks_complete_through_a_fabric() {
    let (_, clock) = run_compute_with_fabric_memory(1);

    assert!(clock.time_now_ns() > 80.0);
}

#[test]
fn compute_tasks_report_cache_hits_backing_traffic_and_completion_time() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Platform::from_string(
        &engine,
        &clock,
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
      num_active_requests: 1
      lsu_access_bytes: {LSU_ACCESS_BYTES}
      compute:
        execution_units:
          vector:
            capabilities:
              - {{ class: add, ops_per_tick: 256 }}

caches:
  - name: c0
    config:
      delay_ticks: 5

memories:
  - name: hbm0
    kind: hbm
    base_address: {MEMORY_BASE_ADDRESS}
    config:
      capacity_bytes: {MEMORY_CAPACITY_BYTES}
      delay_ticks: 20

connections:
  - connect:
    - pe.pe0
    - cache.c0.dev
  - connect:
    - cache.c0.mem
    - mem.hbm0
"
        ),
    )
    .unwrap();

    assert_eq!(platform.num_pes(), 1);
    assert_eq!(platform.num_memories(), 1);
    assert_eq!(platform.num_fabrics(), 0);
    assert_eq!(platform.num_caches(), 1);

    let dispatcher = two_add_tasks_dispatcher();
    platform.attach_dispatcher(&dispatcher);

    run_simulation!(engine);

    // Expect cache hits for repeated tensor reads plus memory traffic for
    // first reads and writes. Each 32B read response with 8B header occupies
    // two 32B/tick service ticks. Fill ingress and shared client response
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
            platform.attach_dispatcher(&two_add_tasks_dispatcher());
            engine.run().unwrap();

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
