// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use super::block_pipeline_tests::{fixture_resources, idle};
use super::*;
use crate::processing_element::block_runtime::BlockExecutionSnapshot;
use crate::processing_element::tensors::Tensor;
use crate::processing_element::tensors::dtype::DataType;

fn view(shape: &[usize], dtype: DataType, addr: u64) -> TensorView {
    TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
}
fn config(
    source: &str,
    sizes: &str,
    inputs: Vec<TensorView>,
    outputs: Vec<TensorView>,
) -> ProgramTaskConfig {
    ProgramTaskConfig {
        id: "generic".into(),
        program: yaml_serde::from_str(source).unwrap(),
        tiling: Some(yaml_serde::from_str(sizes).unwrap()),
        pipeline: Some(
            yaml_serde::from_str(
                "mode: overlap\ninput_buffers: 3\noutput_buffers: 2\nprefetch_distance: 2",
            )
            .unwrap(),
        ),
        inputs,
        outputs,
    }
}
type ResultSnapshot = (
    crate::processing_element::MachineOpCounts,
    usize,
    usize,
    u64,
    BlockExecutionSnapshot,
);
fn run(config: ProgramTaskConfig) -> ResultSnapshot {
    run_with_local(config, None)
}
fn run_with_local(config: ProgramTaskConfig, rate: Option<u64>) -> ResultSnapshot {
    let mut engine = gwr_engine::engine::Engine::default();
    let (mut runner, done, _) = fixture_resources(&mut engine, 12, 2);
    if let Some(bytes_per_tick) = rate {
        let mut limits = runner.block_execution.as_ref().unwrap().config.clone();
        limits.local_memory_service =
            Some(crate::processing_element::block_execution::LocalStagingConfig { bytes_per_tick });
        runner.block_execution = Some(
            crate::processing_element::block_runtime::BlockExecutionResources::new(
                limits,
                &runner.clock,
                &runner.entity,
            )
            .unwrap(),
        );
    }
    let inspect = runner.clone();
    engine.spawn(async move { runner.run_program(0, &config).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    idle(&inspect);
    let s = inspect.stats.borrow();
    (
        s.machine_ops.clone(),
        s.read_bytes,
        s.written_bytes,
        inspect.clock.tick_now().tick(),
        inspect.block_execution.as_ref().unwrap().snapshot(),
    )
}
fn equivalent(config: ProgramTaskConfig) {
    let overlapped = run(config.clone());
    let mut serial = config.clone();
    serial.pipeline.as_mut().unwrap().mode =
        crate::processing_element::program::pipeline::PipelineMode::Serial;
    let serial = run(serial);
    let mut ordinary = config;
    ordinary.pipeline = None;
    let ordinary = run(ordinary);
    assert_eq!(
        (&overlapped.0, overlapped.1, overlapped.2),
        (&serial.0, serial.1, serial.2)
    );
    assert_eq!(
        (&overlapped.0, overlapped.1, overlapped.2),
        (&ordinary.0, ordinary.1, ordinary.2)
    );
    assert_eq!(
        overlapped.4.peak_reserved_window_bytes,
        serial.4.peak_reserved_window_bytes
    );
    assert_eq!(
        (
            serial.4.load_compute_overlap_ticks,
            serial.4.store_compute_overlap_ticks
        ),
        (0, 0)
    );
}
#[test]
fn generic_copy_and_elementwise_keep_the_ordinary_request_and_work_contract() {
    for expr in [
        "{read: {tensor: A, indices: [r,k]}}",
        "{add: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: A, indices: [r,k]}}]}",
    ] {
        equivalent(config(
            &format!(
                "inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [r,k]\n   expr: {expr}"
            ),
            "sizes: {r: 1, k: 3}",
            vec![view(&[3, 7], DataType::Fp32, 0)],
            vec![view(&[3, 7], DataType::Fp32, 8192)],
        ));
    }
}
#[test]
fn packed_contraction_has_odd_rows_and_tail_chunks() {
    equivalent(config(
        "inputs: [A,B]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [m,n]\n   expr: {reduce: {op: sum, over: [k], value: {mul: [{read: {tensor: A, indices: [m,k]}}, {read: {tensor: B, indices: [k,n]}}]}}}",
        "sizes: {m: 1, n: 2, k: 3}",
        vec![
            view(&[2, 7], DataType::Fp16, 0),
            view(&[7, 3], DataType::Int4, 4096),
        ],
        vec![view(&[2, 3], DataType::Fp16, 8192)],
    ));
}
#[test]
fn sum_reduces_multiple_axes_in_order() {
    equivalent(config(
        "inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [m]\n   expr: {reduce: {op: sum, over: [k,l], value: {read: {tensor: A, indices: [m,k,l]}}}}",
        "sizes: {m: 1, k: 2, l: 3}",
        vec![view(&[2, 3, 5], DataType::Fp32, 0)],
        vec![view(&[2], DataType::Fp32, 8192)],
    ));
}
#[test]
fn dependent_assignments_retain_both_reduction_results() {
    equivalent(config(
        "inputs: [A]\noutputs: [Y]\ntemporaries: {S: {dtype: fp32}, M: {dtype: fp32}}\nassignments:\n - output: S\n   axes: [r]\n   expr: {reduce: {op: sum, over: [k], value: {read: {tensor: A, indices: [r,k]}}}}\n - output: M\n   axes: [r]\n   expr: {reduce: {op: max, over: [k], value: {read: {tensor: A, indices: [r,k]}}}}\n - output: Y\n   axes: [r,k]\n   expr: {mul: [{read: {tensor: A, indices: [r,k]}}, {add: [{read: {tensor: S, indices: [r]}}, {read: {tensor: M, indices: [r]}}]}]}",
        "sizes: {r: 1, k: 2}",
        vec![view(&[2, 5], DataType::Fp32, 0)],
        vec![view(&[2, 5], DataType::Fp32, 8192)],
    ));
}
#[test]
fn affine_excluded_stride_windows_work_for_convolution_and_pooling() {
    let read = "{read: {tensor: X, indices: [b,c,{affine: {terms: {y: 2, kh: 1}, offset: -1}},{affine: {terms: {x: 2, kw: 1}, offset: -1}}], bounds: exclude}}";
    let source = format!(
        "inputs: [X,W]\noutputs: [Y]\nextents: {{y: 3, x: 3, kh: 3, kw: 3}}\nassignments:\n - output: Y\n   axes: [b,o,y,x]\n   expr: {{reduce: {{op: sum, over: [c,kh,kw], value: {{mul: [{read}, {{read: {{tensor: W, indices: [o,c,kh,kw]}}}}]}}}}}}"
    );
    equivalent(config(
        &source,
        "sizes: {b: 1, o: 1, y: 2, x: 2, c: 1, kh: 2, kw: 2}",
        vec![
            view(&[1, 1, 5, 5], DataType::Fp32, 0),
            view(&[1, 1, 3, 3], DataType::Fp32, 4096),
        ],
        vec![view(&[1, 1, 3, 3], DataType::Fp32, 8192)],
    ));
    for op in ["min", "max"] {
        let source = format!(
            "inputs: [X]\noutputs: [Y]\nextents: {{y: 3, x: 3, kh: 3, kw: 3}}\nassignments:\n - output: Y\n   axes: [b,c,y,x]\n   expr: {{reduce: {{op: {op}, over: [kh,kw], value: {read}}}}}"
        );
        equivalent(config(
            &source,
            "sizes: {b: 1, c: 1, y: 2, x: 2, kh: 2, kw: 2}",
            vec![view(&[1, 1, 5, 5], DataType::Fp32, 0)],
            vec![view(&[1, 1, 3, 3], DataType::Fp32, 8192)],
        ));
    }
}
#[test]
fn complete_window_is_checked_before_any_effect_and_all_requested_slots_are_charged() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, _, _) = fixture_resources(&mut engine, 12, 2);
    let mut task = config(
        "inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [k]\n   expr: {add: [{read: {tensor: A, indices: [k]}}, {read: {tensor: A, indices: [k]}}]}",
        "sizes: {k: 5}",
        vec![view(&[9], DataType::Fp32, 0)],
        vec![view(&[9], DataType::Fp32, 8192)],
    );
    let p = super::program_pipeline::plan(
        &task,
        &runner.compute_model,
        &runner.block_execution.as_ref().unwrap().config,
    )
    .unwrap();
    assert_eq!(
        p.bytes,
        3 * p.input_slot_bytes + 2 * p.output_slot_bytes + p.working_bytes
    );
    p.validate_capacity(&task, p.bytes).unwrap();
    assert!(p.validate_capacity(&task, p.bytes - 1).is_err());
    task.pipeline.as_mut().unwrap().input_buffers = usize::MAX;
    assert!(
        super::program_pipeline::plan(
            &task,
            &runner.compute_model,
            &runner.block_execution.as_ref().unwrap().config
        )
        .is_err()
    );
    assert_eq!(runner.lsu.issue_snapshot().issued_read_packets, 0);
    idle(&runner);
}

#[test]
fn balanced_reduction_demonstrates_a_real_overlap_speedup() {
    let c = config(
        "inputs: [A,B]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [m,n]\n   expr: {reduce: {op: sum, over: [k], value: {mul: [{read: {tensor: A, indices: [m,k]}}, {read: {tensor: B, indices: [k,n]}}]}}}",
        "sizes: {m: 1, n: 16, k: 8}",
        vec![
            view(&[4, 64], DataType::Fp32, 0),
            view(&[64, 16], DataType::Fp32, 4096),
        ],
        vec![view(&[4, 16], DataType::Fp32, 8192)],
    );
    let overlap = run(c.clone());
    let mut serial = c;
    serial.pipeline.as_mut().unwrap().mode =
        crate::processing_element::program::pipeline::PipelineMode::Serial;
    let serial = run(serial);
    assert_eq!(
        (&overlap.0, overlap.1, overlap.2),
        (&serial.0, serial.1, serial.2)
    );
    assert!(
        overlap.3 < serial.3,
        "overlap {}, serial {}",
        overlap.3,
        serial.3
    );
    assert!(overlap.4.load_compute_overlap_ticks > 0);
    eprintln!(
        "balanced generic reduction: overlap {} ticks, serial {} ticks, actual load/compute overlap {}",
        overlap.3, serial.3, overlap.4.load_compute_overlap_ticks
    );
}

#[test]
fn generic_cancellation_drains_window_input_compute_output_and_ack_ownership() {
    for stop in ["window", "load", "input", "compute", "output", "store"] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, done, _) = fixture_resources(&mut engine, 12, 2);
        let inspect = runner.clone();
        let task = config(
            "inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [r]\n   expr: {reduce: {op: sum, over: [k], value: {read: {tensor: A, indices: [r,k]}}}}",
            "sizes: {r: 1, k: 8}",
            vec![view(&[8, 64], DataType::Fp32, 0)],
            vec![view(&[8], DataType::Fp32, 8192)],
        );
        engine.spawn(async move {
            let resources = runner.block_execution.as_ref().unwrap();
            let window = if stop == "window" {
                Some(runner.sram.reserve(runner.sram_bytes).await?)
            } else {
                None
            };
            let engine_credit = if stop == "input" {
                Some(
                    runner
                        .compute_scheduler
                        .acquire(crate::processing_element::compute::EngineId(1))
                        .await?,
                )
            } else {
                None
            };
            let store = if stop == "output" {
                Some(resources.store.reserve(1).await?)
            } else {
                None
            };
            let mut future = Box::pin(runner.run_program(0, &task));
            let mut reached = false;
            for _ in 0..4000 {
                assert!(
                    futures::poll!(&mut future).is_pending(),
                    "finished before {stop}"
                );
                let s = resources.snapshot();
                reached = match stop {
                    "window" => true,
                    "load" => {
                        s.active_load_chunks > 0
                            && runner.lsu.issue_snapshot().outstanding_packets > 0
                    }
                    "input" => s.occupied_input_slots == 3,
                    "compute" => s.active_compute_chunks > 0,
                    "output" => s.occupied_output_slots == 2 && s.completed_finalisations >= 2,
                    "store" => {
                        s.active_store_tiles > 0
                            && runner.lsu.issue_snapshot().outstanding_packets > 0
                    }
                    _ => unreachable!(),
                };
                if reached {
                    break;
                }
                runner.clock.wait_ticks(1)?.await;
            }
            assert!(reached, "did not reach {stop}: {:?}", resources.snapshot());
            drop(future);
            drop(window);
            drop(engine_credit);
            drop(store);
            runner.clock.wait_ticks(200)?.await;
            assert_eq!(done.0.get(), 0);
            idle(&runner);
            assert_eq!(runner.lsu.issue_snapshot().outstanding_packets, 0);
            Ok(())
        });
        engine.run().unwrap();
        idle(&inspect);
    }
}

#[test]
fn several_boundary_outputs_keep_distinct_live_result_storage() {
    equivalent(config(
        "inputs: [A]\noutputs: [Y,Z]\nassignments:\n - output: Y\n   axes: [r,k]\n   expr: {add: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: A, indices: [r,k]}}]}\n - output: Z\n   axes: [r,k]\n   expr: {mul: [{read: {tensor: Y, indices: [r,k]}}, {read: {tensor: A, indices: [r,k]}}]}",
        "sizes: {r: 1, k: 3}",
        vec![view(&[2, 5], DataType::Fp32, 0)],
        vec![
            view(&[2, 5], DataType::Fp32, 8192),
            view(&[2, 5], DataType::Fp32, 16384),
        ],
    ));
}

#[test]
fn singleton_extremum_chunks_are_distinct_input_generations_even_without_comparisons() {
    for op in ["min", "max"] {
        equivalent(config(
            &format!(
                "inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [r]\n   expr: {{reduce: {{op: {op}, over: [k], value: {{read: {{tensor: A, indices: [r,k]}}}}}}}}"
            ),
            "sizes: {r: 3, k: 1}",
            vec![view(&[3, 2], DataType::Fp32, 0)],
            vec![view(&[3], DataType::Fp32, 8192)],
        ));
    }
}

#[test]
fn intermediate_handoffs_and_consumers_use_the_selected_local_service() {
    let task = config(
        "inputs: [A]\noutputs: [Y]\ntemporaries: {T: {dtype: fp32}}\nassignments:\n - output: T\n   axes: [r,k]\n   expr: {add: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: A, indices: [r,k]}}]}\n - output: Y\n   axes: [r,k]\n   expr: {mul: [{read: {tensor: T, indices: [r,k]}}, {read: {tensor: A, indices: [r,k]}}]}",
        "sizes: {r: 1, k: 2}",
        vec![view(&[2, 4], DataType::Fp32, 0)],
        vec![view(&[2, 4], DataType::Fp32, 8192)],
    );
    let fast = run(task.clone());
    let slow = run_with_local(task, Some(1));
    assert_eq!(
        (slow.4.local_read_bytes, slow.4.local_write_bytes),
        (192, 160)
    );
    assert_eq!(
        (fast.4.local_read_bytes, fast.4.local_write_bytes),
        (192, 160)
    );
    assert_eq!((&fast.0, fast.1, fast.2), (&slow.0, slow.1, slow.2));
    assert!(slow.3 > fast.3);
}

#[test]
fn tile_control_markers_do_not_block_elementwise_prefetch() {
    let task = config(
        "inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [r,k]\n   expr: {add: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: A, indices: [r,k]}}]}",
        "sizes: {r: 1, k: 128}",
        vec![view(&[4, 128], DataType::Fp32, 0)],
        vec![view(&[4, 128], DataType::Fp32, 8192)],
    );
    let result = run(task);
    assert!(result.4.load_compute_overlap_ticks > 0, "{:?}", result.4);
}

#[test]
fn failed_store_stops_later_reduction_updates_and_drains_issued_requests() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, done, _) = fixture_resources(&mut engine, 12, 2);
    let inspect = runner.clone();
    runner.stats.borrow_mut().written_bytes = usize::MAX;
    let task = config(
        "inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [r]\n   expr: {reduce: {op: sum, over: [k], value: {read: {tensor: A, indices: [r,k]}}}}",
        "sizes: {r: 1, k: 8}",
        vec![view(&[4, 512], DataType::Fp32, 0)],
        vec![view(&[4], DataType::Fp32, 16384)],
    );
    engine.spawn(async move { runner.run_program(0, &task).await });
    let error = engine.run().unwrap_err();
    assert!(error.0.contains("write traffic"), "{error}");
    let snapshot = inspect.block_execution.as_ref().unwrap().snapshot();
    assert!(
        snapshot.completed_compute_chunks <= 67,
        "continued dependent updates after store failed: {snapshot:?}"
    );
    assert_eq!(inspect.lsu.issue_snapshot().issued_write_packets, 1);
    assert_eq!(inspect.lsu.issue_snapshot().outstanding_packets, 0);
    assert_eq!(done.0.get(), 0);
    idle(&inspect);
}

#[test]
fn an_empty_initial_copy_does_not_consume_the_next_real_chunk_credit() {
    let mut task = config(
        "inputs: [A,B]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [m,n]\n   expr: {reduce: {op: sum, over: [k], value: {mul: [{read: {tensor: A, indices: [m,k]}}, {read: {tensor: B, indices: [k,n]}}]}}}",
        "sizes: {m: 1, n: 16, k: 8}",
        vec![
            view(&[4, 8], DataType::Fp32, 0),
            view(&[8, 16], DataType::Fp32, 4096),
        ],
        vec![view(&[4, 16], DataType::Fp32, 8192)],
    );
    let p = task.pipeline.as_mut().unwrap();
    p.input_buffers = 2;
    p.output_buffers = 1;
    p.prefetch_distance = 1;
    let result = run(task);
    assert!(result.4.load_compute_overlap_ticks > 0, "{:?}", result.4);
}
