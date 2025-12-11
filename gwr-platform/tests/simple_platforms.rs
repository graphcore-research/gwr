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
use gwr_models::processing_element::task::{MemoryOp, MemoryTaskConfig, Task};
use gwr_platform::Platform;

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
                if self.completed_tasks.borrow().contains(&idx)
                    || self.started_tasks.borrow().contains(&idx)
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
        self.change.notify()?;
        Ok(())
    }

    fn set_task_completed(&self, task_idx: usize) -> gwr_engine::types::SimResult {
        let mut handle = self.completed_tasks.borrow_mut();
        handle.insert(task_idx);
        self.change.notify()?;
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
        self.change.listen().await
    }
}

fn build_dispatcher() -> Rc<dyn Dispatch> {
    Rc::new(TestDispatcher::new(
        HashMap::from([
            (
                0,
                Task::MemoryTask {
                    config: MemoryTaskConfig {
                        op: MemoryOp::Load,
                        addr: 0x1_0000_0000,
                        num_bytes: 128,
                    },
                },
            ),
            (
                1,
                Task::MemoryTask {
                    config: MemoryTaskConfig {
                        op: MemoryOp::Load,
                        addr: 0x1_0000_0000,
                        num_bytes: 128,
                    },
                },
            ),
        ]),
        HashMap::from([("pe0".to_string(), VecDeque::from([0, 1]))]),
    ))
}

macro_rules! pe_mem_config {
    ($num_active_requests:expr) => {
        format!(
            "
processing_elements:
  - name: pe0
    memory_map:
      ranges:
        - base_address: 0x1_0000_0000
          size_bytes: 16GB
          device: hbm0
    config:
      num_active_requests: {}
      lsu_access_bytes: 32

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
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
fn simple_pe_mem_one_request() {
    let (_, clock) = run_pe_mem(1);

    // There are two 128 bytes requested over a 32-byte interface. Each request has
    // a 10ns delay.
    assert_eq!(clock.time_now_ns(), 80.0);
}

#[test]
fn simple_pe_mem_two_requests() {
    let (_, clock) = run_pe_mem(2);

    // There are 128 bytes requested over a 32-byte interface with a 10ns delay, but
    // the LSU supports two outstanding requests. So, the first two access take
    // 11ns, but the remainder take 10ns because they overlap.
    assert_eq!(clock.time_now_ns(), 41.0);
}

#[test]
fn simple_pe_cache_mem() {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Platform::from_string(
        &engine,
        &clock,
        "
processing_elements:
  - name: pe0
    memory_map:
      ranges:
        - base_address: 0x1_0000_0000
          size_bytes: 16GB
          device: hbm0
    config:
      num_active_requests: 1
      lsu_access_bytes: 32

caches:
  - name: c0
    delay_ticks: 5

memories:
  - name: hbm0
    kind: hbm
    base_address: 0x1_0000_0000
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

    // Expect 4 cache misses which need to go to memory (30ns each)
    // and 4 cache hits (5ns each)
    assert_eq!(clock.time_now_ns(), 140.0);
}
