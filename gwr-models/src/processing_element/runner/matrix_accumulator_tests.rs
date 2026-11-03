// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Unexecuted private native accumulator/SRAM/activity controls.
use std::cell::Cell;
use std::future::Future;
use std::task::Poll;

use futures::future::poll_fn;
use gwr_engine::engine::Engine;

use super::matrix_test_fixture::*;
use super::*;
use crate::memory::Memory;
use crate::memory::memory_access::MemoryAccess;
use crate::processing_element::tensors::Tensor;
use crate::processing_element::tensors::dtype::DataType;

const ACCUMULATOR_CAPACITY_BYTES: u64 = 8 * 8 * 4;

fn run_compute_wait_with_capacity(capacity: Option<u64>) -> (u64, usize) {
    let mut engine = Engine::default();
    let (runner, memory, done) = runner_with_accumulator_capacity(&mut engine, capacity, false);
    let inspect = runner.clone();
    let final_inspect = runner.clone();
    let clock = engine.default_clock();
    let finish_clock = clock.clone();
    let end = Rc::new(Cell::new(0));
    let finish = end.clone();
    engine.spawner().spawn(async move {
        runner.run_program(0, &square_matrix_program_task()).await?;
        finish.set(finish_clock.tick_now().tick());
        Ok(())
    });
    engine.spawner().spawn(async move {
        clock.wait_ticks(30)?.await;
        assert_compute_holds_resources(&inspect, capacity);
        Ok(())
    });
    engine.run().unwrap();
    assert_runner_resources_released(&final_inspect);
    assert_eq!(done.0.get(), 1);
    assert_eq!(
        (memory.bytes_read(), memory.bytes_written()),
        (2 * LSU_ACCESS_BYTES, LSU_ACCESS_BYTES)
    );
    let (_, peak, _, _) = final_inspect.compute_scheduler.accumulator_state_for_test();
    assert_eq!(
        peak,
        vec![if capacity.is_some() {
            ACCUMULATOR_CAPACITY_BYTES
        } else {
            0
        }]
    );
    let peak_sram_bytes = final_inspect.stats.borrow().peak_sram_bytes;
    (end.get(), peak_sram_bytes)
}

fn square_matrix_program_task() -> ProgramTaskConfig {
    let mut task = matrix_program_task(8, 8, 16);
    task.id = "skinny".into();
    task
}

fn runner_with_accumulator_capacity(
    engine: &mut Engine,
    capacity: Option<u64>,
    replicated: bool,
) -> (TaskRunner, Rc<Memory<MemoryAccess>>, Rc<Completed>) {
    matrix_runner_fixture(
        engine,
        &matrix_processing_element_config(1, false, capacity, replicated),
    )
}

fn assert_runner_resources_released(runner: &TaskRunner) {
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

fn assert_compute_holds_resources(runner: &TaskRunner, capacity: Option<u64>) {
    let (current, _, used, _) = runner.compute_scheduler.accumulator_state_for_test();
    assert_eq!(
        current,
        vec![if capacity.is_some() {
            ACCUMULATOR_CAPACITY_BYTES
        } else {
            0
        }]
    );
    assert_eq!(used, vec![true]);
    assert!(runner.sram.used() > 0);
    assert_eq!(runner.stats.borrow().read_bytes, 512);
    assert_eq!(runner.stats.borrow().written_bytes, 0);
    assert_eq!(
        runner.activity_lanes.compute[0]
            .borrow()
            .active_count_for_test(),
        1
    );
}

#[test]
fn matrix_service_wait_holds_accumulator_demand_and_sram_until_completion() {
    let without_capacity = run_compute_wait_with_capacity(None);
    let with_capacity = run_compute_wait_with_capacity(Some(ACCUMULATOR_CAPACITY_BYTES));
    assert_eq!(without_capacity, with_capacity);
}

#[test]
fn cancel_polled_runner_service_releases_accumulator_sram_and_activity() {
    let mut engine = Engine::default();
    let (runner, memory, done) =
        runner_with_accumulator_capacity(&mut engine, Some(ACCUMULATOR_CAPACITY_BYTES), false);
    let inspect = runner.clone();
    let final_inspect = runner.clone();
    let clock = engine.default_clock();
    engine.spawner().spawn(async move {
        let task = square_matrix_program_task();
        let mut job = Box::pin(runner.run_program(0, &task));
        for _ in 0..30 {
            poll_fn(|cx| {
                assert!(Future::poll(job.as_mut(), cx).is_pending());
                Poll::Ready(())
            })
            .await;
            clock.wait_ticks(1)?.await;
        }
        assert_eq!(
            inspect.compute_scheduler.accumulator_state_for_test().0,
            vec![ACCUMULATOR_CAPACITY_BYTES]
        );
        assert_eq!(inspect.stats.borrow().read_bytes, 512);
        assert_eq!(inspect.stats.borrow().written_bytes, 0);
        assert!(inspect.sram.used() > 0);
        drop(job);
        assert_runner_resources_released(&inspect);
        // Another task can use the resources released by cancellation.
        runner.run_program(1, &task).await?;
        assert_runner_resources_released(&inspect);
        Ok(())
    });
    engine.run().unwrap();
    assert_runner_resources_released(&final_inspect);
    assert_eq!(done.0.get(), 1);
    assert_eq!(
        (memory.bytes_read(), memory.bytes_written()),
        (4 * LSU_ACCESS_BYTES, LSU_ACCESS_BYTES)
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
        vec![ACCUMULATOR_CAPACITY_BYTES]
    );
}

#[test]
fn matrix_tile_exceeding_accumulator_capacity_is_rejected_before_resource_use() {
    let mut engine = Engine::default();
    let (runner, memory, done) = runner_with_accumulator_capacity(&mut engine, Some(128), false);
    let inspect = runner.clone();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &square_matrix_program_task()).await });
    let error = engine.run().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("matrix accumulator capacity cannot hold"),
        "{error}"
    );
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
    assert_eq!(
        inspect.compute_scheduler.accumulator_state_for_test().1,
        vec![0]
    );
}

#[test]
fn broadcast_matrix_operand_reserves_accumulator_for_the_logical_output() {
    const OUTPUT_EXTENT: usize = 8;
    const REDUCTION_ELEMENTS: usize = 16;
    const BF16_BYTES: usize = 2;
    const FP32_BYTES: usize = 4;
    let mut engine = Engine::default();
    let (runner, memory, done) =
        runner_with_accumulator_capacity(&mut engine, Some(ACCUMULATOR_CAPACITY_BYTES), false);
    let inspect = runner.clone();
    let mut task = square_matrix_program_task();
    task.inputs[1] = TensorView::new_full(
        Tensor::new(&[REDUCTION_ELEMENTS, 1], &DataType::Bf16, RHS_BASE_ADDRESS).unwrap(),
    );
    task.program.extents.insert("n".into(), OUTPUT_EXTENT);
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &task).await });
    engine.run().unwrap();
    assert_runner_resources_released(&inspect);
    assert_eq!(done.0.get(), 1);
    let input_bytes = (OUTPUT_EXTENT * REDUCTION_ELEMENTS + REDUCTION_ELEMENTS) * BF16_BYTES;
    let output_bytes = OUTPUT_EXTENT * OUTPUT_EXTENT * FP32_BYTES;
    assert_eq!(
        (
            inspect.stats.borrow().read_bytes,
            inspect.stats.borrow().written_bytes
        ),
        (input_bytes, output_bytes)
    );
    assert_eq!(
        (memory.bytes_read(), memory.bytes_written()),
        (2 * LSU_ACCESS_BYTES, LSU_ACCESS_BYTES)
    );
    assert_eq!(
        inspect.compute_scheduler.accumulator_state_for_test().1,
        vec![ACCUMULATOR_CAPACITY_BYTES]
    );
}
