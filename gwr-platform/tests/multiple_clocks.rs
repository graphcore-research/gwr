// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::processing_element::MachineOpCounts;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::operators::{OperatorCustom, Tensor, TensorView};
use gwr_models::processing_element::task::{ComputeOp, ComputeTaskConfig, Task};
use gwr_models::test_helpers::{create_default_memory_map, create_read};
use gwr_platform::Platform;

#[test]
fn memories_use_explicit_clocks_and_inherit_the_yaml_default() {
    let mut engine = Engine::default();
    let platform = Platform::from_string(
        &mut engine,
        r"
clocks:
  - {name: slow, frequency_mhz: 500}
  - {name: fast, frequency_mhz: 1000}
default_clock: slow
memory_maps: []
memories:
  - name: inherited
    kind: hbm
    base_address: 0
    config: {capacity_bytes: 1MiB, delay_ticks: 10, bw_bytes_per_tick: 8}
  - name: explicit
    clock: fast
    kind: hbm
    base_address: 0x10_0000
    config: {capacity_bytes: 1MiB, delay_ticks: 10, bw_bytes_per_tick: 8}
",
    )
    .unwrap();

    let completions = Rc::new(RefCell::new(Vec::new()));
    for (name, frequency_mhz, address) in [("inherited", 500.0, 0), ("explicit", 1000.0, 0x10_0000)]
    {
        let memory = platform.memory(name).unwrap();
        let clock = engine.clock_mhz(frequency_mhz);
        let mut tx = OutPort::new(engine.top(), &format!("{name}_request"));
        tx.connect(memory.port_rx()).unwrap();
        let mut rx =
            InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), &format!("{name}_response"));
        memory.connect_port_tx(rx.state()).unwrap();
        let map = Rc::new(create_default_memory_map());
        let request = create_read(engine.top(), &map, 32, address, 0, 8);
        let completions = completions.clone();
        engine.spawn(async move {
            tx.put(request)?.await;
            rx.get()?.await;
            completions.borrow_mut().push((name, clock.time_now_ns()));
            Ok(())
        });
    }

    engine.run().unwrap();
    assert_eq!(
        *completions.borrow(),
        [("explicit", 10.0), ("inherited", 20.0)]
    );
    assert_eq!(engine.time_now_ns(), 20.0);
    assert_eq!(platform.memory("inherited").unwrap().bytes_read(), 32);
    assert_eq!(platform.memory("explicit").unwrap().bytes_read(), 32);
}

struct OneTask {
    task: Task,
    active: Cell<bool>,
    completed: Cell<bool>,
    changed: Repeated<()>,
}

#[async_trait(?Send)]
impl Dispatch for OneTask {
    fn task_by_id(&self, _: usize) -> Result<Task, SimError> {
        Ok(self.task.clone())
    }

    fn set_task_active(&self, _: usize) -> SimResult {
        self.active.set(true);
        self.changed.notify();
        Ok(())
    }

    fn set_task_completed(&self, _: usize) -> SimResult {
        self.completed.set(true);
        self.changed.notify();
        Ok(())
    }

    fn ready_task_indices(&self, _: &str) -> Result<(bool, Vec<usize>), SimError> {
        Ok((
            self.completed.get(),
            if self.active.get() { vec![] } else { vec![0] },
        ))
    }

    async fn wait_for_change(&self) {
        self.changed.listen().await;
    }

    fn total_tasks_for_pe(&self, _: &str) -> usize {
        1
    }
}

fn workload_time(kind: &str, frequencies: [usize; 4]) -> f64 {
    let [pe_mhz, cache_mhz, fabric_mhz, memory_mhz] = frequencies;
    let yaml = format!(
        r"
clocks:
  - {{name: pe_clock, frequency_mhz: {pe_mhz}}}
  - {{name: cache_clock, frequency_mhz: {cache_mhz}}}
  - {{name: fabric_clock, frequency_mhz: {fabric_mhz}}}
  - {{name: memory_clock, frequency_mhz: {memory_mhz}}}
default_clock: memory_clock
memory_maps:
  - {{name: map, devices: [mem]}}
processing_elements:
  - name: pe
    clock: pe_clock
    memory_map: map
    config: {{adds_per_tick: 16, lsu_access_bytes: 32, sram_bytes: 64}}
caches:
  - name: cache
    clock: cache_clock
    config:
      line_size_bytes: 32
      num_sets: 1
      num_ways: 1
      delay_ticks: 2
      bw_bytes_per_tick: 16
fabrics:
  - name: fabric
    clock: fabric_clock
    kind: {kind}
    columns: 2
    rows: 1
    config:
      fabric_ports_per_node: 1
      ticks_per_hop: 2
      ticks_overhead: 1
      port_bits_per_tick: 4096
memories:
  - name: mem
    clock: memory_clock
    kind: hbm
    base_address: 0
    config: {{capacity_bytes: 1MiB, delay_ticks: 8, bw_bytes_per_tick: 32}}
connections:
  - connect: [pe.pe, cache.cache.dev]
  - connect: [cache.cache.mem, 'fabric.fabric@(0,0)']
  - connect: [mem.mem, 'fabric.fabric@(1,0)']
"
    );
    let mut engine = Engine::default();
    let platform = Platform::from_string(&mut engine, &yaml).unwrap();
    let input = Tensor::new(&[8], &DataType::Fp32, 0).unwrap();
    let dispatcher = Rc::new(OneTask {
        task: Task::ComputeTask {
            config: ComputeTaskConfig {
                id: "compute".to_string(),
                op: ComputeOp::Custom(OperatorCustom {
                    name: None,
                    machine_ops: MachineOpCounts {
                        adds: 64,
                        ..MachineOpCounts::default()
                    },
                }),
                inputs: vec![Some(TensorView::new_full(input))],
                outputs: Vec::new(),
            },
        },
        active: Cell::new(false),
        completed: Cell::new(false),
        changed: Repeated::new(()),
    });
    let dispatch: Rc<dyn Dispatch> = dispatcher.clone();
    platform.attach_dispatcher(&dispatch);
    engine.run().unwrap();
    assert!(dispatcher.completed.get());
    assert_eq!(platform.memory("mem").unwrap().bytes_read(), 32);
    assert_eq!(
        platform
            .cache("cache")
            .unwrap()
            .traffic_snapshot()
            .fill_requests,
        1
    );
    engine.time_now_ns()
}

#[test]
fn every_component_clock_controls_mixed_domain_execution() {
    for kind in ["functional", "routed"] {
        let baseline = workload_time(kind, [1000; 4]);
        for (component, frequencies) in [
            ("PE", [500, 1000, 1000, 1000]),
            ("cache", [1000, 500, 1000, 1000]),
            ("fabric", [1000, 1000, 500, 1000]),
            ("memory", [1000, 1000, 1000, 500]),
        ] {
            assert!(
                workload_time(kind, frequencies) > baseline,
                "slowing {component} must increase execution time with a {kind} fabric"
            );
        }
        assert_eq!(workload_time(kind, [500; 4]), baseline * 2.0);
    }
}
