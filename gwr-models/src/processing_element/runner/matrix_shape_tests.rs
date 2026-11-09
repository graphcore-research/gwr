// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Private native SRAM residency and pure geometry guard controls.
use std::cell::Cell;

use async_trait::async_trait;

use super::*;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::memory::{Memory, MemoryConfig};
use crate::processing_element::tensors::Tensor;
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::{ProcessingElement, ProcessingElementConfig};
struct Completed(Cell<usize>);
#[async_trait(?Send)]
impl crate::processing_element::dispatch::Dispatch for Completed {
    fn task_by_id(&self, _: usize) -> Result<Task, SimError> {
        unreachable!()
    }
    fn set_task_active(&self, _: usize) -> SimResult {
        Ok(())
    }
    fn set_task_completed(&self, _: usize) -> SimResult {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
    fn ready_task_indices(&self, _: &str) -> Result<(bool, Vec<usize>), SimError> {
        Ok((true, vec![]))
    }
    async fn wait_for_change(&self) {
        futures::future::pending::<()>().await;
    }
    fn total_tasks_for_pe(&self, _: &str) -> usize {
        0
    }
}
fn program() -> ProgramTaskConfig {
    let view = |shape: &[usize], dtype, base| {
        TensorView::new_full(Tensor::new(shape, &dtype, base).unwrap())
    };
    ProgramTaskConfig{pipeline: None,id:"skinny".into(),tiling:None,program:yaml_serde::from_str("inputs: [A,B]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [m,n]\n   expr: {matrix_mac: {over: [k], execution: {instruction: tensor, accumulator: fp32, sparsity: dense}, args: [{read: {tensor: A, indices: [m,k]}}, {read: {tensor: B, indices: [k,n]}}]}}\n").unwrap(),inputs:vec![view(&[1,16],DataType::Bf16,0),view(&[16,256],DataType::Bf16,65536)],outputs:vec![view(&[1,256],DataType::Fp32,131_072)]}
}
fn fixture(
    engine: &mut gwr_engine::engine::Engine,
    enabled: bool,
    replicated: bool,
    boundary: Option<u64>,
) -> (TaskRunner, Rc<Memory<MemoryAccess>>, Rc<Completed>) {
    let clock = engine.default_clock();
    let mut map = MemoryMap::new();
    map.insert(0, 262_144, gwr_engine::types::DeviceId(0))
        .unwrap();
    let map = Rc::new(map);
    let policy = if enabled {
        ", matrix_service_tile: {m: 16,n: 16,k: 16}"
    } else {
        ""
    };
    let engine_cfg = format!(
        "capabilities: [{{class: matrix,input_dtypes: [[bf16,bf16]],ops_per_tick: 256,execution: {{instruction: tensor,accumulator: fp32,sparsity: dense}}{policy}}}]"
    );
    let compute = if replicated {
        format!("engines: {{matrix0: {{{engine_cfg}}}, matrix1: {{{engine_cfg}}}}}")
    } else {
        format!("engines: {{matrix0: {{{engine_cfg}}}}}")
    };
    let cfg = ProcessingElementConfig {
        block_execution: None,
        num_active_requests: 16,
        lsu_access_bytes: 65536,
        overhead_size_bytes: 0,
        sram_bytes: 65536,
        boundary_payload_service: boundary,
        local_payload_service: None,
        compute: yaml_serde::from_str(&compute).unwrap(),
        operation_classes: yaml_serde::from_str("matrix: {operations: [matrix_mac],arity: 2}")
            .unwrap(),
    };
    let pe = ProcessingElement::new_and_register(
        engine,
        &clock,
        engine.top(),
        "pe",
        &map,
        &cfg,
        gwr_engine::types::DeviceId(1),
    )
    .unwrap();
    let memory = Memory::new_and_register(
        engine,
        &clock,
        engine.top(),
        "memory",
        MemoryConfig::new(0, 262_144, 65536, 0),
    )
    .unwrap();
    pe.connect_port_tx(memory.port_rx()).unwrap();
    memory.connect_port_tx(pe.port_rx()).unwrap();
    let done = Rc::new(Completed(Cell::new(0)));
    let dispatcher: Dispatcher = done.clone();
    pe.set_dispatcher(&dispatcher);
    let runner = TaskRunner {
        block_execution: None,
        entity: pe.entity.clone(),
        clock,
        dispatcher,
        lsu: pe.lsu.clone(),
        compute_model: pe.compute_model.clone(),
        compute_scheduler: pe.compute_scheduler.clone(),
        sram: pe.sram.clone(),
        sram_bytes: cfg.sram_bytes,
        boundary_payload: pe.boundary_payload.clone(),
        local_payload: pe.local_payload.clone(),
        stats: pe.stats.clone(),
        activity_lanes: pe.activity_lanes.clone(),
        flop_monitor: None,
    };
    (runner, memory, done)
}
#[test]
fn service_wait_extends_residency_without_padding_capacity_or_payload() {
    let mut finishes = Vec::new();
    for enabled in [false, true] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, done) = fixture(&mut engine, enabled, false, None);
        let inspect = runner.clone();
        let final_inspect = runner.clone();
        let end = Rc::new(Cell::new(0));
        let observed_end = end.clone();
        let clock = engine.default_clock();
        let finish_clock = clock.clone();
        engine.spawner().spawn(async move {
            runner.run_program(0, &program()).await?;
            observed_end.set(finish_clock.tick_now().tick());
            Ok(())
        });
        engine.spawner().spawn(async move {
            clock.wait_ticks(30)?.await;
            assert_eq!(inspect.sram.used(), if enabled { 9248 } else { 0 });
            assert_eq!(
                inspect.stats.borrow().written_bytes,
                if enabled { 0 } else { 1024 }
            );
            Ok(())
        });
        engine.run().unwrap();
        assert_eq!(done.0.get(), 1);
        finishes.push(end.get());
        println!(
            "direct runner enabled={enabled} completed_tick={}",
            end.get()
        );
        assert_eq!(final_inspect.sram.used(), 0);
        assert_eq!(final_inspect.stats.borrow().peak_sram_bytes, 9248);
        assert_eq!(
            (memory.bytes_read(), memory.bytes_written()),
            (131_072, 65536)
        );
    }
    // Direct runner completion precedes the dispatcher engine shutdown tick.
    assert_eq!(finishes[0], 17);
    assert_eq!(finishes[1], finishes[0] + 240);
}
#[test]
fn unsupported_broadcast_rejects_before_sram_or_boundary_effects() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done) = fixture(&mut engine, true, false, Some(1));
    let inspect = runner.clone();
    let mut p = program();
    p.inputs[1] = TensorView::new_full(Tensor::new(&[16, 1], &DataType::Bf16, 65536).unwrap());
    // Preserve the full output axis instead of inferring n=1 from B.
    p.program.extents.insert("n".into(), 256);
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &p).await });
    let error = engine.run().unwrap_err();
    println!("broadcast rejection: {error}");
    assert!(error.0.contains("matrix service requires"), "{error}");
    assert_eq!(done.0.get(), 0);
    assert_eq!(inspect.sram.used(), 0);
    assert_eq!(inspect.stats.borrow().peak_sram_bytes, 0);
    assert_eq!(
        (
            inspect.stats.borrow().read_bytes,
            inspect.stats.borrow().written_bytes
        ),
        (0, 0)
    );
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
    let boundary = inspect.boundary_payload.as_ref().unwrap().snapshot();
    assert_eq!(
        (
            boundary.admitted_read_bytes,
            boundary.admitted_write_bytes,
            boundary.completed_read_bytes,
            boundary.completed_write_bytes,
            boundary.canceled_read_bytes,
            boundary.canceled_write_bytes
        ),
        (0, 0, 0, 0, 0, 0)
    );
    assert_eq!(
        inspect
            .stats
            .borrow()
            .machine_ops
            .checked_operation_total()
            .unwrap(),
        0
    );
}

#[test]
fn actual_shared_engine_waits_while_independent_candidate_replicas_overlap() {
    let mut elapsed = Vec::new();
    for replicas in [false, true] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, done) = fixture(&mut engine, true, replicas, None);
        let inspect = runner.clone();
        for id in 0..2 {
            let runner = runner.clone();
            engine
                .spawner()
                .spawn(async move { runner.run_program(id, &program()).await });
        }
        engine.run().unwrap();
        assert_eq!(done.0.get(), 2);
        assert_eq!(
            inspect.stats.borrow().machine_ops.by_operation["matrix_mac"],
            8192
        );
        assert_eq!(
            (memory.bytes_read(), memory.bytes_written()),
            (262_144, 131_072)
        );
        assert_eq!(inspect.sram.used(), 0);
        elapsed.push(engine.default_clock().tick_now().tick());
    }
    assert!(
        elapsed[0] > elapsed[1],
        "same-engine service must contend: {elapsed:?}"
    );
    println!("native shared/independent matrix service ticks {elapsed:?}");
}
#[test]
fn streamed_preflight_checks_actual_chunk_not_fictitious_full_service() {
    let k = 1usize << 40;
    let mut task = program();
    let view = |shape: &[usize], dtype, base| {
        TensorView::new_full(Tensor::new(shape, &dtype, base).unwrap())
    };
    task.inputs = vec![
        view(&[1, k], DataType::Bf16, 0),
        view(&[k, 1], DataType::Bf16, 1u64 << 43),
    ];
    task.outputs = vec![view(&[1, 1], DataType::Fp32, 1u64 << 44)];
    task.tiling = Some(yaml_serde::from_str("sizes: {k: 1}").unwrap());
    let region = validate_program(
        &task.program,
        &RegionBindings {
            inputs: task.inputs.clone(),
            outputs: task.outputs.clone(),
        },
    )
    .unwrap();
    let config=yaml_serde::from_str("engines: {matrix: {capabilities: [{class: matrix,input_dtypes: [[bf16,bf16]],ops_per_tick: 0.000000001,execution: {instruction: tensor,accumulator: fp32,sparsity: dense},matrix_service_tile: {m: 16,n: 16,k: 16}}]}}").unwrap();
    let classes = yaml_serde::from_str("matrix: {operations: [matrix_mac],arity: 2}").unwrap();
    let model = ComputeModel::new(&config, &classes).unwrap();
    let schedule = plan_tiles(&region, task.tiling.as_ref()).unwrap();
    preflight_program(&model, &region, &schedule).unwrap();
    let full = matrix_shape::work(
        &model,
        &region.program.assignments[0],
        &region,
        &std::collections::BTreeMap::new(),
    )
    .unwrap()
    .unwrap();
    assert!(
        model.lower(&full).is_err(),
        "fictitious one-shot service overflows; no giant native run"
    );
}
#[test]
fn explicit_exclude_is_rejected_by_existing_validator_with_or_without_policy() {
    for enabled in [false, true] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, _, _) = fixture(&mut engine, enabled, false, None);
        let mut task = program();
        let mut value = yaml_serde::to_value(&task.program).unwrap();
        value["assignments"][0]["expr"]["matrix_mac"]["args"][0]["read"]["bounds"] =
            "exclude".into();
        task.program = yaml_serde::from_value(value).unwrap();
        assert!(
            validate_program(
                &task.program,
                &RegionBindings {
                    inputs: task.inputs.clone(),
                    outputs: task.outputs.clone()
                }
            )
            .is_err()
        );
        assert_eq!(runner.sram.used(), 0);
        assert_eq!(
            runner
                .stats
                .borrow()
                .machine_ops
                .checked_operation_total()
                .unwrap(),
            0
        );
    }
}

#[test]
fn asymmetric_macro_resolves_operand_roles_not_output_axis_order() {
    let mut task = program();
    let mut value = yaml_serde::to_value(&task.program).unwrap();
    value["assignments"][0]["axes"] = yaml_serde::from_str("[n,m]").unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][0]["read"]["indices"] =
        yaml_serde::from_str("[k,m]").unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][1]["read"]["indices"] =
        yaml_serde::from_str("[n,k]").unwrap();
    task.program = yaml_serde::from_value(value).unwrap();
    let view = |shape: &[usize], dtype, base| {
        TensorView::new_full(Tensor::new(shape, &dtype, base).unwrap())
    };
    task.inputs = vec![
        view(&[5, 2], DataType::Bf16, 0),
        view(&[3, 5], DataType::Bf16, 65536),
    ];
    task.outputs = vec![view(&[3, 2], DataType::Fp32, 131_072)];
    let region = validate_program(
        &task.program,
        &RegionBindings {
            inputs: task.inputs,
            outputs: task.outputs,
        },
    )
    .unwrap();
    let config=yaml_serde::from_str("engines: {matrix: {capabilities: [{class: matrix,input_dtypes: [[bf16,bf16]],ops_per_tick: 1,execution: {instruction: tensor,accumulator: fp32,sparsity: dense},matrix_service_tile: {m: 2,n: 4,k: 1}}]}}").unwrap();
    let classes = yaml_serde::from_str("matrix: {operations: [matrix_mac],arity: 2}").unwrap();
    let model = ComputeModel::new(&config, &classes).unwrap();
    let work = matrix_shape::work(
        &model,
        &region.program.assignments[0],
        &region,
        &std::collections::BTreeMap::new(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        work.stages[0].operations[0].matrix_shape,
        Some(crate::processing_element::compute::MatrixWorkShape { m: 2, n: 3, k: 5 })
    );
    assert_eq!(model.lower(&work).unwrap().service_ticks().unwrap(), 40);
    assert_eq!(work.stages[0].operations[0].count, 30);
}

fn batched_program() -> ProgramTaskConfig {
    let mut task = program();
    task.program = yaml_serde::from_str("inputs: [A,B]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [b,n,m]\n   expr: {matrix_mac: {over: [k], execution: {instruction: tensor, accumulator: fp32, sparsity: dense}, args: [{read: {tensor: A, indices: [b,k,m]}}, {read: {tensor: B, indices: [n,b,k]}}]}}\n").unwrap();
    let view = |shape: &[usize], dtype, base| {
        TensorView::new_full(Tensor::new(shape, &dtype, base).unwrap())
    };
    task.inputs = vec![
        view(&[2, 5, 2], DataType::Bf16, 0),
        view(&[3, 2, 5], DataType::Bf16, 65536),
    ];
    task.outputs = vec![view(&[2, 3, 2], DataType::Fp32, 131_072)];
    task.tiling = Some(yaml_serde::from_str("sizes: {b: 1}").unwrap());
    task
}

fn batch_model(capacity: u64) -> ComputeModel {
    let config = yaml_serde::from_str(&format!("engines: {{matrix: {{matrix_accumulator_capacity_bytes: {capacity}, capabilities: [{{class: matrix,input_dtypes: [[bf16,bf16]],ops_per_tick: 1,execution: {{instruction: tensor,accumulator: fp32,sparsity: dense}},matrix_service_tile: {{m: 2,n: 4,k: 1}}}}]}}}}")).unwrap();
    let classes = yaml_serde::from_str("matrix: {operations: [matrix_mac],arity: 2}").unwrap();
    ComputeModel::new(&config, &classes).unwrap()
}

#[test]
fn singleton_batch_tiles_keep_matrix_roles_counts_and_accumulator_capacity() {
    let task = batched_program();
    let region = validate_program(
        &task.program,
        &RegionBindings {
            inputs: task.inputs.clone(),
            outputs: task.outputs.clone(),
        },
    )
    .unwrap();
    let model = batch_model(24);
    let schedule = plan_tiles(&region, task.tiling.as_ref()).unwrap();
    preflight_program(&model, &region, &schedule).unwrap();
    let mut tiles = 0;
    for tile in schedule.tiles() {
        let work = matrix_shape::work(
            &model,
            &region.program.assignments[0],
            &region,
            &tile.output_intervals,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            work.stages[0].operations[0].matrix_shape,
            Some(crate::processing_element::compute::MatrixWorkShape { m: 2, n: 3, k: 5 })
        );
        assert_eq!(work.stages[0].operations[0].count, 30);
        assert_eq!(model.lower(&work).unwrap().service_ticks().unwrap(), 40);
        assert!(
            batch_model(23)
                .lower(&work)
                .unwrap_err()
                .0
                .contains("accumulator capacity")
        );
        tiles += 1;
    }
    assert_eq!(tiles, 2);
}

#[test]
fn batch_intervals_larger_than_one_reject_before_service() {
    let mut task = batched_program();
    task.tiling = Some(yaml_serde::from_str("sizes: {b: 2}").unwrap());
    let region = validate_program(
        &task.program,
        &RegionBindings {
            inputs: task.inputs.clone(),
            outputs: task.outputs.clone(),
        },
    )
    .unwrap();
    let model = batch_model(1024);
    let schedule = plan_tiles(&region, task.tiling.as_ref()).unwrap();
    let error = preflight_program(&model, &region, &schedule).unwrap_err();
    assert!(
        error.0.contains("singleton resolved batch intervals"),
        "{error}"
    );
    assert!(
        matrix_shape::work(
            &model,
            &region.program.assignments[0],
            &region,
            &std::collections::BTreeMap::new()
        )
        .is_err()
    );
}

#[test]
fn native_runner_executes_two_singleton_batch_tiles() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, _, done) = fixture(&mut engine, true, false, None);
    let inspect = runner.clone();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &batched_program()).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    assert_eq!(
        inspect.stats.borrow().machine_ops.by_operation["matrix_mac"],
        60
    );
    assert_eq!(inspect.stats.borrow().written_bytes, 48);
    assert_eq!(inspect.sram.used(), 0);
}
