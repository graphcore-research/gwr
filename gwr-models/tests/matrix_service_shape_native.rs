// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Real PE/LSU controls for optional caller matrix service geometry.
use std::cell::Cell;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::types::{DeviceId, SimError, SimResult};
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::memory_map::MemoryMap;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::task::{ProgramTaskConfig, Task};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_models::processing_element::{ProcessingElement, ProcessingElementConfig};
struct OneTask {
    task: Task,
    state: Cell<u8>,
}
#[async_trait(?Send)]
impl Dispatch for OneTask {
    fn task_by_id(&self, _: usize) -> Result<Task, SimError> {
        Ok(self.task.clone())
    }
    fn set_task_active(&self, _: usize) -> SimResult {
        self.state.set(1);
        Ok(())
    }
    fn set_task_completed(&self, _: usize) -> SimResult {
        self.state.set(2);
        Ok(())
    }
    fn ready_task_indices(&self, _: &str) -> Result<(bool, Vec<usize>), SimError> {
        Ok((
            self.state.get() != 0,
            if self.state.get() == 0 {
                vec![0]
            } else {
                vec![]
            },
        ))
    }
    async fn wait_for_change(&self) {
        futures::future::pending::<()>().await;
    }
    fn total_tasks_for_pe(&self, _: &str) -> usize {
        1
    }
}
fn matrix(m: usize, n: usize, k: usize) -> ProgramTaskConfig {
    let view = |shape: &[usize], dtype, base| {
        TensorView::new_full(Tensor::new(shape, &dtype, base).unwrap())
    };
    ProgramTaskConfig { pipeline: None, id:"matrix".into(), tiling:None, program:yaml_serde::from_str(r"
inputs: [A, B]
outputs: [Y]
assignments:
 - output: Y
   axes: [m, n]
   expr: {matrix_mac: {over: [k], execution: {instruction: tensor, accumulator: fp32, sparsity: dense}, args: [{read: {tensor: A, indices: [m, k]}}, {read: {tensor: B, indices: [k, n]}}]}}
").unwrap(), inputs:vec![view(&[m,k],DataType::Bf16,0),view(&[k,n],DataType::Bf16,65536)],outputs:vec![view(&[m,n],DataType::Fp32,131_072)] }
}
struct Observed {
    result: Result<(), SimError>,
    ticks: u64,
    completed: u8,
    ops: usize,
    traffic: (usize, usize),
    backing: (usize, usize),
}
fn execute(task: ProgramTaskConfig, enabled: bool) -> Observed {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let mut map = MemoryMap::new();
    map.insert(0, 262_144, DeviceId(0)).unwrap();
    let map = Rc::new(map);
    let policy = if enabled {
        ", matrix_service_tile: {m: 16, n: 16, k: 16}"
    } else {
        ""
    };
    let config=ProcessingElementConfig {block_execution: None,num_active_requests:16,lsu_access_bytes:65536,overhead_size_bytes:0,sram_bytes:65536,boundary_payload_service:None,local_payload_service:None,compute:yaml_serde::from_str(&format!("engines: {{matrix: {{capabilities: [{{class: matrix, input_dtypes: [[bf16,bf16]], ops_per_tick: 256, execution: {{instruction: tensor, accumulator: fp32, sparsity: dense}}{policy}}}]}}}}" )).unwrap(),operation_classes:yaml_serde::from_str("matrix: {operations: [matrix_mac], arity: 2}").unwrap()};
    let pe = ProcessingElement::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "pe",
        &map,
        &config,
        DeviceId(1),
    )
    .unwrap();
    let memory = Memory::<MemoryAccess>::new_and_register(
        &engine,
        &clock,
        engine.top(),
        "hbm",
        MemoryConfig::new(0, 262_144, 65536, 0),
    )
    .unwrap();
    pe.connect_port_tx(memory.port_rx()).unwrap();
    memory.connect_port_tx(pe.port_rx()).unwrap();
    let dispatch = Rc::new(OneTask {
        task: Task::ProgramTask {
            config: Box::new(task),
        },
        state: Cell::new(0),
    });
    let dyn_dispatch: Rc<dyn Dispatch> = dispatch.clone();
    pe.set_dispatcher(&dyn_dispatch);
    let result = engine.run();
    Observed {
        result,
        ticks: clock.tick_now().tick(),
        completed: dispatch.state.get(),
        ops: pe.machine_ops().checked_operation_total().unwrap(),
        traffic: pe.memory_traffic_bytes(),
        backing: (memory.bytes_read(), memory.bytes_written()),
    }
}
fn run(m: usize, n: usize, k: usize, enabled: bool) -> (u64, usize, usize) {
    let observed = execute(matrix(m, n, k), enabled);
    observed.result.unwrap();
    assert_eq!(observed.completed, 2);
    assert_eq!(observed.ops, m * n * k);
    let traffic = (2 * (m * k + k * n), 4 * m * n);
    assert_eq!(observed.traffic, traffic);
    assert_eq!(observed.backing, (131_072, 65536));
    println!(
        "matrix m={m} n={n} k={k} enabled={enabled} ticks={} MAC={} read={} write={}",
        observed.ticks,
        m * n * k,
        traffic.0,
        traffic.1
    );
    (observed.ticks, traffic.0, traffic.1)
}

#[test]
fn equal_macs_square_and_skinny_use_actual_runner_geometry() {
    let plain_square = run(16, 16, 16, false);
    let plain_skinny = run(1, 256, 16, false);
    assert_eq!(plain_square.0, 18, "retained pre-integration None baseline");
    assert_eq!(
        plain_square.0, plain_skinny.0,
        "one coalesced packet per operand preserves external timing control"
    );
    let square = run(16, 16, 16, true);
    let skinny = run(1, 256, 16, true);
    assert_eq!(square.0, plain_square.0);
    assert_eq!(skinny.0, square.0 + 240);
    assert_eq!((square.1, square.2), (1024, 1024));
    assert_eq!((skinny.1, skinny.2), (8224, 1024));
}

#[test]
fn broadcast_and_shared_free_axis_reject_before_external_payload_or_compute() {
    for kind in ["broadcast", "shared"] {
        let mut config = matrix(16, 16, 16);
        config.program.extents.extend([
            (String::from("m"), 16),
            (String::from("n"), 16),
            (String::from("k"), 16),
        ]);
        if kind == "broadcast" {
            config.inputs[0] =
                TensorView::new_full(Tensor::new(&[1, 16], &DataType::Bf16, 0).unwrap());
        } else {
            let mut value = yaml_serde::to_value(&config.program).unwrap();
            value["assignments"][0]["expr"]["matrix_mac"]["args"][1]["read"]["indices"] =
                yaml_serde::from_str("[m,k]").unwrap();
            config.program = yaml_serde::from_value(value).unwrap();
        }
        let plain = execute(config.clone(), false);
        plain.result.unwrap();
        assert_eq!(plain.completed, 2);
        let enabled = execute(config, true);
        assert!(
            enabled
                .result
                .unwrap_err()
                .0
                .contains("matrix service requires")
        );
        assert_eq!(enabled.ops, 0);
        assert_eq!(enabled.traffic, (0, 0));
        assert_eq!(enabled.backing, (0, 0));
        assert_ne!(enabled.completed, 2);
    }
}
#[test]
fn actual_reduction_chunks_round_independently_without_extra_payload_or_macs() {
    let mut chunked = matrix(16, 16, 32);
    chunked.tiling = Some(yaml_serde::from_str("sizes: {k: 17}").unwrap());
    let region = gwr_models::processing_element::program::validate::validate_program(
        &chunked.program,
        &gwr_models::processing_element::program::validate::RegionBindings {
            inputs: chunked.inputs.clone(),
            outputs: chunked.outputs.clone(),
        },
    )
    .unwrap();
    let schedule =
        gwr_models::processing_element::program::tile::plan_tiles(&region, chunked.tiling.as_ref())
            .unwrap();
    let tile = schedule.tiles().next().unwrap();
    assert_eq!(tile.output_intervals["m"], 0..16);
    assert_eq!(tile.output_intervals["n"], 0..16);
    let chunks = tile
        .reduction_chunks_for(&["k".into()])
        .map(|chunk| chunk["k"].clone())
        .collect::<Vec<_>>();
    assert_eq!(chunks, vec![0..17, 17..32]);
    println!("actual reduction chunk intervals {chunks:?}");
    let baseline = execute(chunked.clone(), false);
    baseline.result.unwrap();
    let selected = execute(chunked, true);
    selected.result.unwrap();
    assert_eq!(selected.ticks, baseline.ticks + 16);
    assert_eq!(selected.ops, 8192);
    assert_eq!(selected.traffic, (2048, 1024));
    assert_eq!(selected.backing, (2_228_224, 65536));
    assert_eq!(run(16, 16, 32, true).0, run(16, 16, 32, false).0);
    println!(
        "chunked K17+15 logical8192 service12288 baseline_ticks={} selected_ticks={}",
        baseline.ticks, selected.ticks
    );
}
#[test]
fn transposed_direct_operands_and_tails_use_symbolic_roles() {
    let mut task = matrix(2, 3, 5);
    let mut value = yaml_serde::to_value(&task.program).unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][0]["read"]["indices"] =
        yaml_serde::from_str("[k,m]").unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][1]["read"]["indices"] =
        yaml_serde::from_str("[n,k]").unwrap();
    task.program = yaml_serde::from_value(value).unwrap();
    task.inputs[0] = TensorView::new_full(Tensor::new(&[5, 2], &DataType::Bf16, 0).unwrap());
    task.inputs[1] = TensorView::new_full(Tensor::new(&[3, 5], &DataType::Bf16, 65536).unwrap());
    let plain = execute(task.clone(), false);
    plain.result.unwrap();
    let shaped = execute(task, true);
    shaped.result.unwrap();
    assert_eq!(shaped.ops, 30);
    assert_eq!(shaped.traffic, (50, 24));
    assert_eq!(shaped.backing, (131_072, 65536));
    assert_eq!(shaped.ticks, plain.ticks + 15);
}

#[test]
fn batched_legacy_task_is_rejected_before_effects_only_when_policy_enabled() {
    let mut task = matrix(16, 16, 16);
    let mut value = yaml_serde::to_value(&task.program).unwrap();
    value["assignments"][0]["axes"] = yaml_serde::from_str("[b,m,n]").unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][0]["read"]["indices"] =
        yaml_serde::from_str("[b,m,k]").unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][1]["read"]["indices"] =
        yaml_serde::from_str("[b,k,n]").unwrap();
    task.program = yaml_serde::from_value(value).unwrap();
    task.inputs[0] = TensorView::new_full(Tensor::new(&[2, 16, 16], &DataType::Bf16, 0).unwrap());
    task.inputs[1] =
        TensorView::new_full(Tensor::new(&[2, 16, 16], &DataType::Bf16, 65536).unwrap());
    task.outputs[0] =
        TensorView::new_full(Tensor::new(&[2, 16, 16], &DataType::Fp32, 131_072).unwrap());
    let plain = execute(task.clone(), false);
    plain.result.unwrap();
    assert_eq!(plain.ops, 8192);
    assert_eq!(plain.completed, 2);
    assert_eq!(plain.traffic, (2048, 2048));
    let selected = execute(task, true);
    assert!(
        selected
            .result
            .unwrap_err()
            .0
            .contains("matrix service requires")
    );
    assert_eq!(selected.ops, 0);
    assert_eq!(selected.traffic, (0, 0));
    assert_eq!(selected.backing, (0, 0));
    assert_ne!(selected.completed, 2);
}
