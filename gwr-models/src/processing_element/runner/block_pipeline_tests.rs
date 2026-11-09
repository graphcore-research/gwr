// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Adversarial controls using the real runner, LSU, memory and scheduler.
use std::cell::Cell;

use async_trait::async_trait;

use super::*;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::memory::{Memory, MemoryConfig};
use crate::processing_element::tensors::Tensor;
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::{ProcessingElement, ProcessingElementConfig};
pub(super) struct Done(pub(super) Cell<usize>);
#[async_trait(?Send)]
impl crate::processing_element::dispatch::Dispatch for Done {
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
fn fixture_with_memory(
    engine: &mut gwr_engine::engine::Engine,
    delay: usize,
) -> (TaskRunner, Rc<Done>, Rc<Memory<MemoryAccess>>) {
    fixture_resources(engine, delay, 32)
}
pub(super) fn fixture_resources(
    engine: &mut gwr_engine::engine::Engine,
    delay: usize,
    credits: usize,
) -> (TaskRunner, Rc<Done>, Rc<Memory<MemoryAccess>>) {
    let clock = engine.default_clock();
    let mut map = MemoryMap::new();
    map.insert(0, 65536, gwr_engine::types::DeviceId(0))
        .unwrap();
    let map = Rc::new(map);
    let cfg=ProcessingElementConfig{num_active_requests:credits,lsu_access_bytes:64,overhead_size_bytes:0,sram_bytes:8192,boundary_payload_service:None,local_payload_service:None,
 block_execution:Some(yaml_serde::from_str("max_compute_tiles: 1\nmax_load_chunks: 1\nmax_store_tiles: 1\ntransfer_port: lsu\naccumulator_storage: shared_sram").unwrap()),
 compute:yaml_serde::from_str("engines: {matrix: {matrix_accumulator_capacity_bytes: 256, capabilities: [{class: matrix, input_dtypes: [[fp8,fp8]], ops_per_tick: 1024, execution: {instruction: test, accumulator: fp32}}]}, vector: {capabilities: [{class: scalar, ops_per_tick: 4}, {class: cast, input_dtypes: [[fp32]], ops_per_tick: 1024}]}}").unwrap(),
 operation_classes:yaml_serde::from_str("scalar: {operations: [mul, add, compare], arity: 2}\nmatrix: {operations: [matrix_mac], arity: 2}\ncast: {operations: [convert], arity: 1, flops_per_op: 0}").unwrap()};
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
    let memory = Memory::<MemoryAccess>::new_and_register(
        engine,
        &clock,
        engine.top(),
        "memory",
        MemoryConfig::new(0, 65536, 65536, delay),
    )
    .unwrap();
    pe.connect_port_tx(memory.port_rx()).unwrap();
    memory.connect_port_tx(pe.port_rx()).unwrap();
    let done = Rc::new(Done(Cell::new(0)));
    let dispatcher: Dispatcher = done.clone();
    pe.set_dispatcher(&dispatcher);
    let runner = TaskRunner {
        entity: pe.entity.clone(),
        clock,
        dispatcher,
        lsu: pe.lsu.clone(),
        compute_model: pe.compute_model.clone(),
        compute_scheduler: pe.compute_scheduler.clone(),
        sram: pe.sram.clone(),
        sram_bytes: 8192,
        block_execution: pe.block_execution.clone(),
        boundary_payload: None,
        local_payload: None,
        stats: pe.stats.clone(),
        activity_lanes: pe.activity_lanes.clone(),
        flop_monitor: None,
    };
    (runner, done, memory)
}
pub(super) fn fixture(
    engine: &mut gwr_engine::engine::Engine,
    delay: usize,
) -> (TaskRunner, Rc<Done>) {
    let (runner, done, _memory) = fixture_with_memory(engine, delay);
    (runner, done)
}
pub(super) fn program(inputs: usize, outputs: usize, distance: usize) -> ProgramTaskConfig {
    let view = |shape: &[usize], dtype, addr| {
        TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
    };
    ProgramTaskConfig{id:"blocked".into(),pipeline:Some(yaml_serde::from_str(&format!("input_buffers: {inputs}\noutput_buffers: {outputs}\nprefetch_distance: {distance}")).unwrap()),tiling:Some(yaml_serde::from_str("sizes: {b: 1, m: 8, n: 8, k: 4}").unwrap()),
 program:yaml_serde::from_str("inputs: [A,B]\noutputs: [C]\ntemporaries: {P: {dtype: fp32}}\nassignments:\n - output: P\n   axes: [b,m,n]\n   expr: {matrix_mac: {over: [k], execution: {instruction: test, accumulator: fp32}, args: [{read: {tensor: A, indices: [b,m,k]}}, {read: {tensor: B, indices: [b,k,n]}}]}}\n - output: C\n   axes: [b,m,n]\n   expr: {convert: [{read: {tensor: P, indices: [b,m,n]}}]}").unwrap(),
 inputs:vec![view(&[6,8,16],DataType::Fp8,0),view(&[6,16,8],DataType::Fp8,4096)],outputs:vec![view(&[6,8,8],DataType::Fp16,8192)]}
}
pub(super) fn idle(runner: &TaskRunner) {
    let r = runner.block_execution.as_ref().unwrap();
    assert_eq!(runner.sram.used(), 0);
    assert_eq!((r.load.used(), r.compute.used(), r.store.used()), (0, 0, 0));
    let snapshot = r.snapshot();
    assert_eq!(
        (
            snapshot.occupied_input_slots,
            snapshot.occupied_output_slots,
            snapshot.reserved_window_bytes
        ),
        (0, 0, 0)
    );
}
#[test]
fn many_output_slots_queue_and_finalisation_waits_for_store_capacity() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done) = fixture(&mut engine, 0);
    let inspect = runner.clone();
    let after = runner.clone();
    let clock = runner.clock.clone();
    let r = runner.block_execution.as_ref().unwrap().clone();
    // Acquire before polling pipeline: model another PE job occupying the store
    // service.
    let permit = futures::executor::block_on(r.store.reserve(1)).unwrap();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &program(5, 4, 4)).await });
    engine.spawner().spawn(async move {
        clock.wait_ticks(200)?.await;
        let s = r.snapshot();
        assert_eq!(s.occupied_output_slots, 4);
        assert_eq!(s.completed_finalisations, 4);
        assert_eq!(s.completed_store_tiles, 0);
        assert!(s.completed_compute_chunks >= 20);
        assert!(s.peak_occupied_input_slots <= 5);
        assert!(inspect.sram.used() > 0);
        drop(permit);
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    assert_eq!(
        after
            .block_execution
            .as_ref()
            .unwrap()
            .snapshot()
            .completed_store_tiles,
        6
    );
    idle(&after);
}
#[test]
fn stalled_engine_limits_prefetch_to_distance_without_holding_load_job() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done) = fixture(&mut engine, 0);
    let inspect = runner.clone();
    let after = runner.clone();
    let clock = runner.clock.clone();
    let r = runner.block_execution.as_ref().unwrap().clone();
    let reservation = futures::executor::block_on(
        runner
            .compute_scheduler
            .acquire(crate::processing_element::compute::EngineId(0)),
    )
    .unwrap();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &program(5, 1, 2)).await });
    engine.spawner().spawn(async move {
        clock.wait_ticks(100)?.await;
        let s = r.snapshot();
        assert_eq!(s.occupied_input_slots, 3);
        assert_eq!(s.completed_load_chunks, 3);
        assert_eq!(s.completed_compute_chunks, 0);
        assert_eq!(r.load.used(), 0);
        assert!(inspect.sram.used() > 0);
        drop(reservation);
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    idle(&after);
}
#[test]
fn retained_accumulator_does_not_hold_engine_between_late_chunks() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done) = fixture(&mut engine, 30);
    let inspect = runner.clone();
    let after = runner.clone();
    let clock = runner.clock.clone();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &program(2, 1, 1)).await });
    engine.spawner().spawn(async move {
        for _ in 0..1000 {
            clock.wait_ticks(1)?.await;
            let s = inspect.block_execution.as_ref().unwrap().snapshot();
            if s.completed_compute_chunks == 1 && s.completed_load_chunks == 1 {
                let (_, _, busy, _) = inspect.compute_scheduler.accumulator_state_for_test();
                assert_eq!(
                    inspect.compute_scheduler.retained_state_for_test().0[0],
                    256
                );
                assert!(!busy[0]);
                let other = inspect
                    .compute_scheduler
                    .acquire(crate::processing_element::compute::EngineId(0))
                    .await?;
                clock.wait_ticks(2)?.await;
                drop(other);
                return Ok(());
            }
        }
        Err(SimError(
            "never observed late chunk with idle engine".into(),
        ))
    });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    idle(&after);
}
#[test]
fn cancellation_during_load_retains_packet_custody_then_drains_without_completion() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done) = fixture(&mut engine, 30);
    let inspect = runner.clone();
    let after = runner.clone();
    let clock = runner.clock.clone();
    engine.spawner().spawn(async move {
        let config = program(2, 1, 1);
        let mut task = Box::pin(runner.run_program(0, &config));
        for _ in 0..10 {
            futures::future::poll_fn(|cx| {
                assert!(std::future::Future::poll(task.as_mut(), cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            clock.wait_ticks(1)?.await;
        }
        drop(task);
        assert!(inspect.sram.used() > 0);
        assert_eq!(inspect.block_execution.as_ref().unwrap().load.used(), 1);
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 0);
    assert_eq!(after.stats.borrow().written_bytes, 0);
    idle(&after);
    assert_eq!(
        after
            .block_execution
            .as_ref()
            .unwrap()
            .snapshot()
            .cancelled_load_chunks,
        1
    );
}

#[test]
fn actual_tracker_activity_names_preserve_pipeline_identifiers_without_groups() {
    struct Capture(Rc<RefCell<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let tracker: gwr_track::Tracker = Rc::new(gwr_track::tracker::TextTracker::new(
        gwr_track::tracker::EntityManager::new(log::Level::Trace),
        Box::new(Capture(bytes.clone())),
    ));
    let mut engine = gwr_engine::engine::Engine::new(&tracker);
    let (runner, done) = fixture(&mut engine, 0);
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &program(2, 1, 1)).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    let text = String::from_utf8(bytes.borrow().clone()).unwrap();
    // Inspect emitted activity events only: group registration must not satisfy
    // these assertions because Perfetto deliberately omits group descriptors.
    let names = text
        .lines()
        .filter(|line| line.contains("activity begin"))
        .collect::<Vec<_>>();
    for expected in [
        "block load blocked output 0 chunk 0 input slot 0 generation 0",
        "block compute blocked output 0 chunk 0 input slot 0 generation 0",
        "block compute blocked output 0 finalise slot 0 generation 0",
        "block store blocked output 0 store slot 0 generation 0",
        "block load blocked output 1 chunk 0 input slot 0 generation 2",
        "block SRAM wait blocked pipeline window",
    ] {
        assert!(
            names.iter().any(|line| line.contains(expected)),
            "missing activity {expected}"
        );
    }
}

#[test]
fn compute_accounting_failure_stops_work_and_drains_inflight_loads() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done, memory) = fixture_with_memory(&mut engine, 30);
    let inspect = runner.clone();
    runner
        .stats
        .borrow_mut()
        .machine_ops
        .by_operation
        .insert("matrix_mac".into(), usize::MAX);
    let error = Rc::new(RefCell::new(None));
    let observed = error.clone();
    engine.spawner().spawn(async move {
        let result = runner.run_program(0, &program(2, 1, 1)).await;
        *observed.borrow_mut() = Some(result.unwrap_err().to_string());
        // Permit the engine to drain the already-issued request workers.
        Ok(())
    });
    engine.run().unwrap();
    assert!(
        error
            .borrow()
            .as_ref()
            .unwrap()
            .contains("machine operation count overflows")
    );
    assert_eq!(done.0.get(), 0);
    let snapshot = inspect.block_execution.as_ref().unwrap().snapshot();
    assert_eq!(snapshot.cancelled_compute_chunks, 1);
    assert_eq!(snapshot.completed_compute_chunks, 0);
    assert!(snapshot.completed_load_chunks >= 1 && snapshot.completed_load_chunks <= 2);
    assert_eq!(snapshot.completed_finalisations, 0);
    assert_eq!(snapshot.completed_store_tiles, 0);
    assert_eq!(memory.bytes_written(), 0);
    assert!(memory.bytes_read() > 0);
    assert_eq!(
        inspect.stats.borrow().machine_ops.by_operation["matrix_mac"],
        usize::MAX
    );
    assert!(
        inspect
            .compute_scheduler
            .retained_state_for_test()
            .0
            .iter()
            .all(|bytes| *bytes == 0)
    );
    idle(&inspect);
}

#[test]
fn partially_issued_store_failure_drains_real_ack_and_does_not_complete() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done, memory) = fixture_with_memory(&mut engine, 30);
    let inspect = runner.clone();
    let mut config = program(2, 1, 1);
    // The first output packet maps; its second packet does not. Call the
    // private executor with a pure plan to exercise a runtime transport
    // error that public preflight correctly rejects, without adding a
    // fault-injection API.
    config.outputs[0] =
        TensorView::new_full(Tensor::new(&[6, 8, 8], &DataType::Fp16, 65536 - 64).unwrap());
    let plan = crate::processing_element::program::block_plan::plan_block_pipeline(
        &config,
        &runner.compute_model,
        &runner.block_execution.as_ref().unwrap().config,
    )
    .unwrap();
    let error = Rc::new(RefCell::new(None));
    let observed = error.clone();
    engine.spawner().spawn(async move {
        let result = runner.run_block_pipeline(&config, plan).await;
        if result.is_ok() {
            runner.dispatcher.set_task_completed(0)?;
        }
        *observed.borrow_mut() = Some(result.unwrap_err().to_string());
        Ok(())
    });
    engine.run().unwrap();
    assert!(
        error
            .borrow()
            .as_ref()
            .unwrap()
            .contains("Unmapped LSU address")
    );
    assert_eq!(done.0.get(), 0);
    // One real write was issued before mapping failed, and its ACK settled.
    assert_eq!(memory.bytes_written(), 64);
    assert_eq!(inspect.stats.borrow().written_bytes, 0);
    let snapshot = inspect.block_execution.as_ref().unwrap().snapshot();
    assert_eq!(snapshot.cancelled_store_tiles, 1);
    assert_eq!(snapshot.completed_store_tiles, 0);
    assert!(snapshot.completed_compute_chunks >= 4 && snapshot.completed_compute_chunks < 24);
    assert!(snapshot.completed_load_chunks < 24);
    assert!(
        inspect
            .compute_scheduler
            .retained_state_for_test()
            .0
            .iter()
            .all(|bytes| *bytes == 0)
    );
    idle(&inspect);
}

#[test]
fn scalar_reduction_uses_stages_and_complete_window() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done) = fixture(&mut engine, 20);
    let inspect = runner.clone();
    let mut config = program(3, 2, 2);
    config.program = yaml_serde::from_str("inputs: [A,B]\noutputs: [C]\ntemporaries: {P: {dtype: fp32}}\nassignments:\n - output: P\n   axes: [b,m,n]\n   expr: {reduce: {op: sum, over: [k], value: {mul: [{read: {tensor: A, indices: [b,m,k]}}, {read: {tensor: B, indices: [b,k,n]}}]}}}\n - output: C\n   axes: [b,m,n]\n   expr: {convert: [{read: {tensor: P, indices: [b,m,n]}}]}").unwrap();
    engine.spawn(async move { runner.run_program(0, &config).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    let s = inspect.block_execution.as_ref().unwrap().snapshot();
    assert_eq!((s.input_buffers, s.output_buffers), (3, 2));
    assert!(s.completed_load_chunks > 0 && s.completed_store_tiles == 6);
    assert!(s.load_compute_overlap_ticks > 0);
    assert_eq!(s.accumulator_bytes, 0);
    idle(&inspect);
}

#[test]
fn broadcast_result_store_overlaps_next_tile_compute() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done, _) = fixture_resources(&mut engine, 20, 1);
    let inspect = runner.clone();
    let mut config = program(2, 1, 1);
    config.program = yaml_serde::from_str("inputs: [A]\noutputs: [C]\nextents: {m: 8, n: 8}\nassignments:\n - output: C\n   axes: [b,m,n]\n   expr: {convert: [{read: {tensor: A, indices: [b,0]}}]}").unwrap();
    config.inputs = vec![TensorView::new_full(
        Tensor::new(&[6, 1], &DataType::Fp32, 0).unwrap(),
    )];
    config.tiling = Some(yaml_serde::from_str("sizes: {b: 1, m: 8, n: 8}").unwrap());
    engine.spawn(async move { runner.run_program(0, &config).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    let s = inspect.block_execution.as_ref().unwrap().snapshot();
    assert!(s.store_compute_overlap_ticks > 0, "{s:?}");
    idle(&inspect);
}

#[test]
fn generic_native_matrix_keeps_real_accumulator_capacity_through_its_handoff() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done) = fixture(&mut engine, 20);
    let inspect = runner.clone();
    let mut config = program(2, 1, 1);
    config.program = yaml_serde::from_str("inputs: [A,B]\noutputs: [C]\ntemporaries: {P: {dtype: fp32}, Q: {dtype: fp16}}\nassignments:\n - output: P\n   axes: [m,n]\n   expr: {matrix_mac: {over: [k], execution: {instruction: test, accumulator: fp32}, args: [{read: {tensor: A, indices: [m,k]}}, {read: {tensor: B, indices: [k,n]}}]}}\n - output: Q\n   axes: [m,n]\n   expr: {convert: [{read: {tensor: P, indices: [m,n]}}]}\n - output: C\n   axes: [m,n]\n   expr: {add: [{read: {tensor: Q, indices: [m,n]}}, {read: {tensor: Q, indices: [m,n]}}]}").unwrap();
    config.tiling = Some(yaml_serde::from_str("sizes: {m: 8, n: 8, k: 4}").unwrap());
    config.inputs = vec![
        TensorView::new_full(Tensor::new(&[8, 16], &DataType::Fp8, 0).unwrap()),
        TensorView::new_full(Tensor::new(&[16, 8], &DataType::Fp8, 4096).unwrap()),
    ];
    config.outputs = vec![TensorView::new_full(
        Tensor::new(&[8, 8], &DataType::Fp16, 8192).unwrap(),
    )];
    engine.spawn(async move { runner.run_program(0, &config).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    let (retained, peak) = inspect.compute_scheduler.retained_state_for_test();
    assert_eq!(retained, vec![0, 0]);
    assert_eq!(peak, vec![256, 0]);
    assert_eq!(
        inspect
            .block_execution
            .as_ref()
            .unwrap()
            .snapshot()
            .accumulator_bytes,
        256
    );
    idle(&inspect);
}
