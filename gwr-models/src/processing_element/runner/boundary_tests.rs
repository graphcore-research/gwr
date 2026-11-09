// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Module-local real runner probes; no public snapshot/residency API.
use std::cell::Cell;

use async_trait::async_trait;

use super::*;
use crate::cache::{Cache, CacheConfig};
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
type RunnerFixture = (
    TaskRunner,
    Rc<Memory<MemoryAccess>>,
    Rc<Completed>,
    Option<Rc<Cache<MemoryAccess>>>,
);

fn fixture(
    engine: &mut gwr_engine::engine::Engine,
    rate: Option<u64>,
    capacity: usize,
    cached: bool,
) -> RunnerFixture {
    fixture_named(engine, rate, capacity, cached, "pe", "hbm")
}
fn fixture_named(
    engine: &mut gwr_engine::engine::Engine,
    rate: Option<u64>,
    capacity: usize,
    cached: bool,
    pe_name: &str,
    memory_name: &str,
) -> RunnerFixture {
    let clock = engine.default_clock();
    let mut map = MemoryMap::new();
    map.insert(0, 4096, gwr_engine::types::DeviceId(0)).unwrap();
    let map = Rc::new(map);
    let cfg = ProcessingElementConfig {
        block_execution: None,
        num_active_requests: 16,
        lsu_access_bytes: 64,
        overhead_size_bytes: 0,
        sram_bytes: capacity,
        boundary_payload_service: rate,
        local_payload_service: None,
        compute: yaml_serde::from_str(
            "engines: {alu: {capabilities: [{class: binary, ops_per_tick: 1000}, {class: unary, ops_per_tick: 1000}]}}",
        )
        .unwrap(),
        operation_classes: yaml_serde::from_str("binary: {operations: [add, mul, compare], arity: 2}\nunary: {operations: [convert], arity: 1}").unwrap(),
    };
    let pe = ProcessingElement::new_and_register(
        engine,
        &clock,
        engine.top(),
        pe_name,
        &map,
        &cfg,
        gwr_engine::types::DeviceId(1),
    )
    .unwrap();
    let memory = Memory::new_and_register(
        engine,
        &clock,
        engine.top(),
        memory_name,
        MemoryConfig::new(0, 4096, 65536, 0),
    )
    .unwrap();
    let cache = if cached {
        let cache = Cache::new_and_register(
            engine,
            &clock,
            engine.top(),
            "cache",
            CacheConfig::new(32, 65536, 16, 2, 0),
        )
        .unwrap();
        pe.connect_port_tx(cache.port_dev_rx()).unwrap();
        cache.connect_port_dev_tx(pe.port_rx()).unwrap();
        cache.connect_port_mem_tx(memory.port_rx()).unwrap();
        memory.connect_port_tx(cache.port_mem_rx()).unwrap();
        Some(cache)
    } else {
        pe.connect_port_tx(memory.port_rx()).unwrap();
        memory.connect_port_tx(pe.port_rx()).unwrap();
        None
    };
    let completed = Rc::new(Completed(Cell::new(0)));
    let dispatcher: crate::processing_element::Dispatcher = completed.clone();
    pe.set_dispatcher(&dispatcher);
    let runner = TaskRunner {
        block_execution: None,
        entity: pe.entity.clone(),
        clock,
        dispatcher: completed.clone(),
        lsu: pe.lsu.clone(),
        compute_model: pe.compute_model.clone(),
        compute_scheduler: pe.compute_scheduler.clone(),
        sram: pe.sram.clone(),
        sram_bytes: capacity,
        boundary_payload: pe.boundary_payload.clone(),
        local_payload: pe.local_payload.clone(),
        stats: pe.stats.clone(),
        activity_lanes: pe.activity_lanes.clone(),
        flop_monitor: None,
    };
    (runner, memory, completed, cache)
}
fn copy(elements: usize, tile: usize) -> ProgramTaskConfig {
    ProgramTaskConfig{pipeline: None,id:"copy".into(),program:yaml_serde::from_str("inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [i]\n   expr: {read: {tensor: A, indices: [i]}}\n").unwrap(),
        tiling:Some(yaml_serde::from_str(&format!("sizes: {{i: {tile}}}")).unwrap()),
        inputs:vec![TensorView::new_full(Tensor::new(&[elements],&DataType::Int8,0).unwrap())],
        outputs:vec![TensorView::new_full(Tensor::new(&[elements],&DataType::Int8,1024).unwrap())]}
}
#[test]
fn boundary_runner_holds_exact_tile_capacity_during_input_wait_and_drains() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done, _) = fixture(&mut engine, Some(1), 128, false);
    let config = copy(128, 64);
    let inspect = runner.clone();
    let final_inspect = inspect.clone();
    let mem = memory.clone();
    let clock = engine.default_clock();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &config).await });
    engine.spawner().spawn(async move {
        clock.wait_ticks(10)?.await;
        assert_eq!(inspect.sram.used(), 128);
        assert_eq!(inspect.stats.borrow().read_bytes, 64);
        assert_eq!(
            mem.bytes_read(),
            64,
            "next tile must not enter while first tile owns all capacity"
        );
        assert_eq!(mem.bytes_written(), 0, "output must await input staging");
        let s = inspect.boundary_payload.as_ref().unwrap().snapshot();
        assert_eq!(
            (
                s.admitted_write_bytes,
                s.completed_write_bytes,
                s.admitted_read_bytes
            ),
            (64, 0, 0)
        );
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    assert_eq!(final_inspect.sram.used(), 0);
    assert_eq!(final_inspect.stats.borrow().peak_sram_bytes, 128);
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (128, 128));
    let s = final_inspect.boundary_payload.as_ref().unwrap().snapshot();
    assert_eq!(
        (
            s.admitted_write_bytes,
            s.completed_write_bytes,
            s.admitted_read_bytes,
            s.completed_read_bytes
        ),
        (128, 128, 128, 128)
    );
    assert_eq!((s.canceled_read_bytes, s.canceled_write_bytes), (0, 0));
}

#[test]
fn repeated_cache_hits_charge_returned_boundary_bytes_not_backing_fill_bytes() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done, cache) = fixture(&mut engine, Some(1), 128, true);
    let inspect = runner.clone();
    engine.spawner().spawn(async move {
        let config = copy(64, 64);
        runner.run_program(0, &config).await?;
        runner.run_program(1, &config).await
    });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 2);
    let cache = cache.unwrap();
    assert_eq!((cache.num_misses(), cache.num_hits()), (1, 1));
    assert_eq!(cache.payload_bytes_read(), 128);
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (64, 128));
    assert_eq!(
        (
            inspect.stats.borrow().read_bytes,
            inspect.stats.borrow().written_bytes
        ),
        (128, 128)
    );
    let s = inspect.boundary_payload.as_ref().unwrap().snapshot();
    assert_eq!(
        (s.completed_write_bytes, s.completed_read_bytes),
        (128, 128)
    );
    assert_eq!(inspect.sram.used(), 0);
}

#[test]
fn failed_external_input_admits_no_local_bytes_but_failed_output_keeps_completed_stage() {
    for access in [AccessType::ReadRequest, AccessType::WriteNonPostedRequest] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, _, _) = fixture(&mut engine, Some(1), 128, false);
        let inspect = runner.clone();
        engine.spawner().spawn(async move {
            let group = runner.activity_lanes.create_group("invalid boundary");
            runner
                .staged_access_ranges(
                    access,
                    std::iter::once(4096u128..4100u128),
                    4,
                    &runner.activity_lanes.lsu_read,
                    "invalid",
                    &group,
                )
                .await
        });
        assert!(engine.run().is_err());
        let s = inspect.boundary_payload.as_ref().unwrap().snapshot();
        assert_eq!(
            (
                s.admitted_write_bytes,
                s.completed_write_bytes,
                s.canceled_write_bytes
            ),
            (0, 0, 0)
        );
        let expected = if access == AccessType::WriteNonPostedRequest {
            4
        } else {
            0
        };
        assert_eq!(
            (
                s.admitted_read_bytes,
                s.completed_read_bytes,
                s.canceled_read_bytes
            ),
            (expected, expected, 0)
        );
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
        assert_eq!(inspect.sram.used(), 0);
    }
}

#[test]
fn canceling_actual_tile_during_boundary_wait_releases_capacity_and_retains_debt() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, _, _) = fixture(&mut engine, Some(1), 128, false);
    let inspect = runner.clone();
    let clock = engine.default_clock();
    engine.spawner().spawn(async move {
        let config = copy(64, 64);
        let work = runner.run_program(0, &config);
        futures::pin_mut!(work);
        let cancel = clock.wait_ticks(10)?;
        futures::pin_mut!(cancel);
        match select(work, cancel).await {
            Either::Left(_) => panic!("tile completed before selected slow stage"),
            Either::Right((_, _pending)) => {}
        }
        // pin_mut retains the original future until scope exit; the runner
        // and its tile reservation are dropped when this actor returns.
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(inspect.sram.used(), 0);
    let service = inspect.boundary_payload.as_ref().unwrap();
    let s = service.snapshot();
    assert_eq!(
        (
            s.admitted_write_bytes,
            s.completed_write_bytes,
            s.canceled_write_bytes
        ),
        (64, 0, 64)
    );
    assert_eq!((s.admitted_read_bytes, s.completed_read_bytes), (0, 0));
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (64, 0));
    assert_eq!(
        inspect.stats.borrow().read_bytes,
        64,
        "completed external input remains real PE traffic after local cancellation"
    );
    let next = service.reserve(BoundaryPayloadDirection::Write, 1).unwrap();
    assert!(
        next.deadline_tick() >= 65,
        "cancellation must retain already-reserved debt"
    );
    drop(next);
}

#[test]
fn local_admission_error_preserves_already_completed_external_input() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, _, _) = fixture(&mut engine, Some(1), 128, false);
    let inspect = runner.clone();
    let service = inspect.boundary_payload.as_ref().unwrap().clone();
    // Synthetic private guard seed, not a physical transaction. On 64-bit
    // targets the next deadline exceeds u64; otherwise the admitted counter
    // overflows. Both must reject before any new local admission commit.
    drop(
        service
            .reserve(BoundaryPayloadDirection::Write, usize::MAX)
            .unwrap(),
    );
    let seeded = service.snapshot();
    engine.spawner().spawn(async move {
        let config = copy(64, 64);
        runner.run_program(0, &config).await
    });
    let error = engine.run().unwrap_err();
    assert!(
        error.0.contains("boundary payload deadline")
            || error.0.contains("boundary admitted payload counter"),
        "{error}"
    );
    assert_eq!(service.snapshot(), seeded);
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (64, 0));
    let stats = inspect.stats.borrow();
    assert_eq!((stats.read_bytes, stats.written_bytes), (64, 0));
    assert_eq!(inspect.sram.used(), 0);
}

fn assert_program_boundary(
    config: ProgramTaskConfig,
    expected: (usize, usize),
    physical: (usize, usize),
) {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done, _) = fixture(&mut engine, Some(2), 4096, false);
    let inspect = runner.clone();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &config).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    let stats = inspect.stats.borrow();
    assert_eq!((stats.read_bytes, stats.written_bytes), expected);
    assert_eq!((memory.bytes_read(), memory.bytes_written()), physical);
    let s = inspect.boundary_payload.as_ref().unwrap().snapshot();
    assert_eq!(
        (s.admitted_write_bytes, s.completed_write_bytes),
        (expected.0, expected.0)
    );
    assert_eq!(
        (s.admitted_read_bytes, s.completed_read_bytes),
        (expected.1, expected.1)
    );
    assert_eq!((s.canceled_read_bytes, s.canceled_write_bytes), (0, 0));
    assert_eq!(inspect.sram.used(), 0);
}
#[test]
fn packed_offset_metadata_and_unused_padding_charge_actual_access_bytes() {
    let mut cfg = copy(3, 3);
    cfg.inputs[0] =
        TensorView::new(Tensor::new(&[8], &DataType::Fp4, 0).unwrap(), &[3], &[1]).unwrap();
    cfg.outputs[0] =
        TensorView::new(Tensor::new(&[8], &DataType::Fp4, 1024).unwrap(), &[3], &[1]).unwrap();
    cfg.program.inputs.extend(["Meta".into(), "Unused".into()]);
    cfg.program.metadata_inputs.push("Meta".into());
    cfg.inputs.push(TensorView::new_full(
        Tensor::new(&[2], &DataType::Int64, 64).unwrap(),
    ));
    cfg.inputs.push(TensorView::new_full(
        Tensor::new(&[4], &DataType::Fp32, 128).unwrap(),
    ));
    // Odd-nibble 3-value views touch2 bytes; declared metadata16 bytes. The
    // unused16-byte binding and parent padding do not imply local accesses.
    assert_program_boundary(cfg, (18, 2), (128, 64));
}
#[test]
fn phase_reread_and_regular_reduction_chunks_stage_every_actual_reload() {
    let program=yaml_serde::from_str(r"
inputs: [A]
outputs: [Y]
temporaries: {S: {dtype: fp32}}
assignments:
 - output: S
   axes: [r]
   expr: {reduce: {op: sum, over: [k], value: {mul: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: A, indices: [r,k]}}]}}}
 - output: Y
   axes: [r,k]
   expr: {mul: [{read: {tensor: A, indices: [r,k]}}, {read: {tensor: S, indices: [r]}}]}
").unwrap();
    let view = |addr| TensorView::new_full(Tensor::new(&[1, 4], &DataType::Fp32, addr).unwrap());
    let cfg = ProgramTaskConfig {
        pipeline: None,
        id: "phase".into(),
        program,
        tiling: Some(yaml_serde::from_str("sizes: {k: 2}").unwrap()),
        inputs: vec![view(0)],
        outputs: vec![view(1024)],
    };
    // Two output tiles each stream the full16-byte reduction row, then
    // reread8 bytes for the phase. External output is16 bytes total.
    assert_program_boundary(cfg, (48, 16), (384, 128));
}
#[test]
fn affine_reduction_windows_and_extrema_stage_only_present_chunk_bytes() {
    for expression in [
        "{reduce: {op: sum, over: [k], value: {mul: [{read: {tensor: A, indices: [{affine: {terms: {y: 1, k: 1}, offset: -1}}], bounds: exclude}}, {read: {tensor: B, indices: [k]}}]}}}",
        "{reduce: {op: max, over: [k], value: {read: {tensor: A, indices: [{affine: {terms: {y: 1, k: 1}, offset: -1}}], bounds: exclude}}}}",
        "{reduce: {op: max, over: [k], value: {convert: [{read: {tensor: A, indices: [{affine: {terms: {y: 1, k: 1}, offset: -1}}], bounds: exclude}}]}}}",
    ] {
        let multiplication = expression.contains("tensor: B");
        let names = if multiplication { "[A,B]" } else { "[A]" };
        let program=yaml_serde::from_str(&format!("inputs: {names}\noutputs: [Y]\nextents: {{y: 3, k: 3}}\nassignments:\n - output: Y\n   axes: [y]\n   expr: {expression}\n")).unwrap();
        let view = |addr| TensorView::new_full(Tensor::new(&[3], &DataType::Fp32, addr).unwrap());
        let mut inputs = vec![view(0)];
        if multiplication {
            inputs.push(view(64));
        }
        let cfg = ProgramTaskConfig {
            pipeline: None,
            id: "windows".into(),
            program,
            tiling: Some(yaml_serde::from_str("sizes: {y: 1}").unwrap()),
            inputs,
            outputs: vec![view(1024)],
        };
        // Source windows contain2+3+2 FP32 elements; B contributes3*3
        // elements for multiply reduction. Separate expression kinds cover
        // affine multiply, simple max, and max of an elementwise call.
        assert_program_boundary(
            cfg,
            (if multiplication { 64 } else { 28 }, 12),
            (if multiplication { 384 } else { 192 }, 192),
        );
    }
}

#[test]
fn concurrent_tiles_share_one_pe_service_while_separate_pes_progress_independently() {
    let mut same = gwr_engine::engine::Engine::default();
    let (runner, _, done, _) = fixture(&mut same, Some(1), 256, false);
    let inspect = runner.clone();
    assert!(Rc::ptr_eq(
        runner.boundary_payload.as_ref().unwrap(),
        inspect.boundary_payload.as_ref().unwrap()
    ));
    same.spawner()
        .spawn(async move { runner.run_program(0, &copy(128, 64)).await });
    same.run().unwrap();
    assert_eq!(done.0.get(), 1);
    let shared = same.default_clock().tick_now().tick();
    assert_eq!(inspect.stats.borrow().peak_sram_bytes, 256);
    let mut separate = gwr_engine::engine::Engine::default();
    let (a, ma, da, _) = fixture_named(&mut separate, Some(1), 128, false, "a", "ha");
    let (b, mb, db, _) = fixture_named(&mut separate, Some(1), 128, false, "b", "hb");
    assert!(!Rc::ptr_eq(
        a.boundary_payload.as_ref().unwrap(),
        b.boundary_payload.as_ref().unwrap()
    ));
    separate
        .spawner()
        .spawn(async move { a.run_program(0, &copy(64, 64)).await });
    separate
        .spawner()
        .spawn(async move { b.run_program(0, &copy(64, 64)).await });
    separate.run().unwrap();
    assert_eq!((da.0.get(), db.0.get()), (1, 1));
    for memory in [ma, mb] {
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (64, 64));
    }
    assert!(shared > separate.default_clock().tick_now().tick());
}

#[test]
fn omitted_boundary_policy_preserves_repeat_timing_and_peak_capacity() {
    let mut observations = Vec::new();
    for _ in 0..2 {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, done, _) = fixture(&mut engine, None, 128, false);
        let inspect = runner.clone();
        assert!(runner.boundary_payload.is_none());
        engine
            .spawner()
            .spawn(async move { runner.run_program(0, &copy(128, 64)).await });
        engine.run().unwrap();
        assert_eq!(done.0.get(), 1);
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (128, 128));
        assert_eq!(inspect.sram.used(), 0);
        observations.push((
            engine.default_clock().tick_now().tick(),
            inspect.stats.borrow().peak_sram_bytes,
        ));
    }
    assert_eq!(observations[0], observations[1]);
    assert_eq!(observations[0].1, 128);
}

#[test]
fn unrelated_alu_service_progresses_while_local_payload_wait_is_pending() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, _, _, _) = fixture(&mut engine, Some(1), 128, false);
    let alu = runner.clone();
    let inspect = runner.clone();
    let clock = engine.default_clock();
    let alu_done = Rc::new(Cell::new(false));
    let completed = alu_done.clone();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &copy(64, 64)).await });
    engine.spawner().spawn(async move {
        let cfg = copy(64, 64);
        let region = validate_program(
            &cfg.program,
            &RegionBindings {
                inputs: cfg.inputs.clone(),
                outputs: cfg.outputs.clone(),
            },
        )?;
        let tile = plan_tiles(&region, cfg.tiling.as_ref())?
            .tiles()
            .next()
            .unwrap();
        let group = alu.activity_lanes.create_group("independent ALU control");
        let work = ComputeWork::operation("add", vec![DataType::Fp32, DataType::Fp32], 1000);
        let plan = alu.compute_model.lower(&work)?;
        alu.run_program_plan(&cfg, &tile, "independent", &group, plan)
            .await?;
        completed.set(true);
        Ok(())
    });
    engine.spawner().spawn(async move {
        clock.wait_ticks(10)?.await;
        assert!(
            alu_done.get(),
            "boundary wait must not acquire the compute resource"
        );
        assert_eq!(
            inspect
                .boundary_payload
                .as_ref()
                .unwrap()
                .snapshot()
                .completed_write_bytes,
            0
        );
        Ok(())
    });
    engine.run().unwrap();
}
