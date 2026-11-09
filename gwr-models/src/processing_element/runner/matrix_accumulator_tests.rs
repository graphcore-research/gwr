// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Unexecuted private native accumulator/SRAM/activity controls.
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
        futures::future::pending::<()>().await
    }
    fn total_tasks_for_pe(&self, _: &str) -> usize {
        0
    }
}
fn program() -> ProgramTaskConfig {
    let view = |shape: &[usize], dtype, base| {
        TensorView::new_full(Tensor::new(shape, &dtype, base).unwrap())
    };
    ProgramTaskConfig{pipeline: None,id:"skinny".into(),tiling:None,program:yaml_serde::from_str("inputs: [A,B]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [m,n]\n   expr: {matrix_mac: {over: [k], execution: {instruction: tensor, accumulator: fp32, sparsity: dense}, args: [{read: {tensor: A, indices: [m,k]}}, {read: {tensor: B, indices: [k,n]}}]}}\n").unwrap(),inputs:vec![view(&[8,16],DataType::Bf16,0),view(&[16,8],DataType::Bf16,65536)],outputs:vec![view(&[8,8],DataType::Fp32,131072)]}
}
fn fixture(
    engine: &mut gwr_engine::engine::Engine,
    capacity: Option<u64>,
    replicated: bool,
    boundary: Option<u64>,
) -> (TaskRunner, Rc<Memory<MemoryAccess>>, Rc<Completed>) {
    let clock = engine.default_clock();
    let mut map = MemoryMap::new();
    map.insert(0, 262144, gwr_engine::types::DeviceId(0))
        .unwrap();
    let map = Rc::new(map);
    let policy = "";
    let budget = capacity
        .map(|v| format!("matrix_accumulator_capacity_bytes: {v}, "))
        .unwrap_or_default();
    let engine_cfg = format!(
        "{budget}capabilities: [{{class: matrix,input_dtypes: [[bf16,bf16]],ops_per_tick: 1,execution: {{instruction: tensor,accumulator: fp32,sparsity: dense}}{policy}}}]"
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
        MemoryConfig::new(0, 262144, 65536, 0),
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

fn idle(runner: &TaskRunner) {
    let (current, _, used, pending) = runner.compute_scheduler.accumulator_state_for_test();
    assert!(current.iter().all(|v| *v == 0));
    assert!(used.iter().all(|v| !*v));
    assert_eq!(pending, 0);
    assert_eq!(runner.sram.used(), 0);
    assert_eq!(
        runner
            .activity_lanes
            .compute_wait
            .borrow()
            .active_count_for_test(),
        0
    );
    assert!(
        runner
            .activity_lanes
            .compute
            .iter()
            .all(|lane| lane.borrow().active_count_for_test() == 0)
    );
}
#[test]
fn actual_compute_wait_holds_logical_demand_and_existing_sram_until_completion() {
    let mut results = Vec::new();
    for capacity in [None, Some(256)] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, done) = fixture(&mut engine, capacity, false, None);
        let inspect = runner.clone();
        let final_inspect = runner.clone();
        let clock = engine.default_clock();
        let finish_clock = clock.clone();
        let end = Rc::new(Cell::new(0));
        let finish = end.clone();
        engine.spawner().spawn(async move {
            runner.run_program(0, &program()).await?;
            finish.set(finish_clock.tick_now().tick());
            Ok(())
        });
        engine.spawner().spawn(async move {
            clock.wait_ticks(30)?.await;
            let (current, _, used, _) = inspect.compute_scheduler.accumulator_state_for_test();
            assert_eq!(current, vec![if capacity.is_some() { 256 } else { 0 }]);
            assert_eq!(used, vec![true]);
            assert!(inspect.sram.used() > 0);
            assert_eq!(inspect.stats.borrow().read_bytes, 512);
            assert_eq!(inspect.stats.borrow().written_bytes, 0);
            assert_eq!(
                inspect.activity_lanes.compute[0]
                    .borrow()
                    .active_count_for_test(),
                1
            );
            Ok(())
        });
        engine.run().unwrap();
        idle(&final_inspect);
        assert_eq!(done.0.get(), 1);
        assert_eq!(
            (memory.bytes_read(), memory.bytes_written()),
            (131072, 65536)
        );
        let (_, peak, _, _) = final_inspect.compute_scheduler.accumulator_state_for_test();
        assert_eq!(peak, vec![if capacity.is_some() { 256 } else { 0 }]);
        results.push((end.get(), final_inspect.stats.borrow().peak_sram_bytes));
    }
    assert_eq!(results[0], results[1]);
}
#[test]
fn cancel_polled_runner_service_releases_accumulator_sram_and_activity() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done) = fixture(&mut engine, Some(256), false, None);
    let inspect = runner.clone();
    let final_inspect = runner.clone();
    let clock = engine.default_clock();
    engine.spawner().spawn(async move {
        let task = program();
        let mut job = Box::pin(runner.run_program(0, &task));
        for _ in 0..30 {
            futures::future::poll_fn(|cx| {
                assert!(std::future::Future::poll(job.as_mut(), cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            clock.wait_ticks(1)?.await;
        }
        assert_eq!(
            inspect.compute_scheduler.accumulator_state_for_test().0,
            vec![256]
        );
        assert_eq!(inspect.stats.borrow().read_bytes, 512);
        assert_eq!(inspect.stats.borrow().written_bytes, 0);
        assert!(inspect.sram.used() > 0);
        drop(job);
        idle(&inspect);
        // Retry actual execution through the same runner and memory, not a new
        // scheduler.
        runner.run_program(1, &task).await?;
        idle(&inspect);
        Ok(())
    });
    engine.run().unwrap();
    idle(&final_inspect);
    assert_eq!(done.0.get(), 1);
    assert_eq!(
        (memory.bytes_read(), memory.bytes_written()),
        (262144, 65536)
    );
    assert_eq!(
        (
            final_inspect.stats.borrow().read_bytes,
            final_inspect.stats.borrow().written_bytes
        ),
        (1024, 256)
    );
    assert_eq!(
        final_inspect
            .compute_scheduler
            .accumulator_state_for_test()
            .1,
        vec![256]
    );
}
#[test]
fn oversized_tile_rejects_before_configured_boundary_sram_activity_or_payload() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done) = fixture(&mut engine, Some(128), false, Some(1));
    let inspect = runner.clone();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &program()).await });
    let error = engine.run().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("matrix accumulator capacity cannot hold"),
        "{error}"
    );
    idle(&inspect);
    assert_eq!(done.0.get(), 0);
    assert_eq!(inspect.stats.borrow().peak_sram_bytes, 0);
    assert_eq!(
        (
            inspect.stats.borrow().read_bytes,
            inspect.stats.borrow().written_bytes
        ),
        (0, 0)
    );
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
    assert_eq!(
        inspect
            .stats
            .borrow()
            .machine_ops
            .checked_operation_total()
            .unwrap(),
        0
    );
    let b = inspect.boundary_payload.as_ref().unwrap().snapshot();
    assert_eq!(
        (
            b.admitted_read_bytes,
            b.admitted_write_bytes,
            b.completed_read_bytes,
            b.completed_write_bytes,
            b.canceled_read_bytes,
            b.canceled_write_bytes
        ),
        (0, 0, 0, 0, 0, 0)
    );
    assert_eq!(
        inspect.compute_scheduler.accumulator_state_for_test().1,
        vec![0]
    );
}

#[test]
fn unsupported_geometry_rejects_before_configured_boundary_and_activity() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done) = fixture(&mut engine, Some(256), false, Some(1));
    let inspect = runner.clone();
    let mut task = program();
    task.inputs[1] = TensorView::new_full(Tensor::new(&[16, 1], &DataType::Bf16, 65536).unwrap());
    task.program.extents.insert("n".into(), 8);
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &task).await });
    let error = engine.run().unwrap_err();
    assert!(
        error.to_string().contains("matrix service requires"),
        "{error}"
    );
    idle(&inspect);
    assert_eq!(done.0.get(), 0);
    assert_eq!(inspect.stats.borrow().peak_sram_bytes, 0);
    assert_eq!(
        (
            inspect.stats.borrow().read_bytes,
            inspect.stats.borrow().written_bytes
        ),
        (0, 0)
    );
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
    let b = inspect.boundary_payload.as_ref().unwrap().snapshot();
    assert_eq!(
        (
            b.admitted_read_bytes,
            b.admitted_write_bytes,
            b.completed_read_bytes,
            b.completed_write_bytes,
            b.canceled_read_bytes,
            b.canceled_write_bytes
        ),
        (0, 0, 0, 0, 0, 0)
    );
    assert_eq!(
        inspect.compute_scheduler.accumulator_state_for_test().1,
        vec![0]
    );
}
