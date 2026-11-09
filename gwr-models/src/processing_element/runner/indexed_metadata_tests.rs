// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Unexecuted indexed-metadata native controls; module-local observations only.
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
        local_payload: None,
        stats: pe.stats.clone(),
        activity_lanes: pe.activity_lanes.clone(),
        flop_monitor: None,
        #[cfg(feature = "functional")]
        functional: None,
    };
    (runner, memory, completed, cache)
}

fn plane(indexed: bool, tail: bool) -> ProgramTaskConfig {
    let mut assignments = Vec::new();
    let columns = if tail { vec![2] } else { vec![0, 1, 2] };
    let mut temporaries = Vec::new();
    for column in columns {
        let name = format!("U{column}");
        temporaries.push(format!("{name}: {{dtype: uint8}}"));
        assignments.push(format!("- output: {name}\n  axes: [n]\n  expr: {{read: {{tensor: S, indices: [n, {{affine: {{terms: {{}}, offset: {column}}}}}]}}}}"));
    }
    let chosen = if tail { "U2" } else { "U0" };
    assignments.push(format!(
        "- output: Y\n  axes: [n]\n  expr: {{read: {{tensor: {chosen}, indices: [n]}}}}"
    ));
    let mut program: ProgramSpec = yaml_serde::from_str(&format!(
        "inputs: [S]\nmetadata_inputs: [S]\noutputs: [Y]\ntemporaries: {{{}}}\nassignments:\n{}\n",
        temporaries.join(", "),
        assignments.join("\n")
    ))
    .unwrap();
    if indexed {
        program.indexed_metadata_inputs.push("S".into());
    }
    ProgramTaskConfig {
        pipeline: None,
        id: "plane".into(),
        program,
        tiling: Some(yaml_serde::from_str("sizes: {n: 4}").unwrap()),
        inputs: vec![TensorView::new_full(
            Tensor::new(&[8, 3], &DataType::Uint8, 0).unwrap(),
        )],
        outputs: vec![TensorView::new_full(
            Tensor::new(&[8], &DataType::Uint8, 1024).unwrap(),
        )],
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Observed {
    ticks: u64,
    pe: (usize, usize),
    hbm: (usize, usize),
    peak: usize,
    machine_ops: crate::processing_element::MachineOpCounts,
    cache_read: Option<usize>,
    cache_misses: Option<usize>,
    cache_hits: Option<usize>,
}
fn execute(indexed: bool, tail: bool, cached: bool) -> Observed {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done, cache) = fixture(&mut engine, None, 64, cached);
    let inspect = runner.clone();
    let clock = engine.default_clock();
    let config = plane(indexed, tail);
    let planned = config.planned_traffic_bytes().unwrap();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &config).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    let stats = inspect.stats.borrow();
    let observed = Observed {
        ticks: clock.tick_now().tick(),
        pe: (stats.read_bytes, stats.written_bytes),
        hbm: (memory.bytes_read(), memory.bytes_written()),
        peak: stats.peak_sram_bytes,
        machine_ops: stats.machine_ops.clone(),
        cache_read: cache.as_ref().map(|c| c.payload_bytes_read()),
        cache_misses: cache.as_ref().map(|c| c.num_misses()),
        cache_hits: cache.as_ref().map(|c| c.num_hits()),
    };
    assert_eq!(
        observed.pe, planned,
        "planned and actual union boundary bytes"
    );
    assert_eq!(
        observed.machine_ops,
        crate::processing_element::MachineOpCounts::default()
    );
    assert_eq!(inspect.sram.used(), 0);
    assert!(observed.peak <= 64);
    println!("INDEXED_SCALE_RUN indexed={indexed} tail={tail} cached={cached} {observed:?}");
    observed
}

#[test]
fn full_column_union_requires_actual_partial_plane_payload() {
    let full = execute(false, false, false);
    assert_eq!(full.pe, (48, 8));
    assert_eq!(full.hbm, (128, 128));
    assert_eq!(
        execute(false, false, false),
        full,
        "repeat None timing/traffic/peak/work exact"
    );
    let indexed = execute(true, false, false);
    assert_eq!(
        indexed.pe,
        (24, 8),
        "two four-row tiles must read12B each rather than24B each"
    );
    assert_eq!(indexed.hbm, (128, 128));
    assert_eq!(indexed.peak, full.peak);
    assert_eq!(indexed.machine_ops, full.machine_ops);
}

#[test]
fn tail_only_column_reads_four_bytes_per_tile_preserving_full_arena() {
    let full = execute(false, true, false);
    assert_eq!(full.pe, (48, 8));
    let indexed = execute(true, true, false);
    assert_eq!(indexed.pe, (8, 8));
    assert_eq!(indexed.hbm, (512, 128));
    assert_eq!(indexed.peak, full.peak);
    assert_eq!(indexed.machine_ops, full.machine_ops);
}

#[test]
fn cacheline_fill_is_separate_from_metadata_union_and_reload_payload() {
    let full = execute(false, false, true);
    let indexed = execute(true, false, true);
    assert_eq!(full.pe, (48, 8));
    assert_eq!(indexed.pe, (24, 8));
    assert_eq!(full.cache_read, Some(128));
    assert_eq!(indexed.cache_read, Some(128));
    assert_eq!(full.hbm, (64, 128));
    assert_eq!(
        indexed.hbm,
        (64, 128),
        "two32B lines cover each aligned64B LSU read"
    );
    assert_eq!((full.cache_misses, full.cache_hits), (Some(1), Some(1)));
    assert_eq!(
        (indexed.cache_misses, indexed.cache_hits),
        (Some(1), Some(1))
    );
    assert_eq!(indexed.peak, full.peak);
    assert_eq!(indexed.machine_ops, full.machine_ops);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep complete setup execution and assertions of this scenario together"
)]
fn invalid_policy_rejects_before_any_backing_boundary_sram_or_activity_effects() {
    execute(true, false, false); // A real valid native baseline must succeed first.
    for invalid in ["duplicate", "unknown", "constant_row", "native_scale_dtype"] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, done, _) = fixture(&mut engine, Some(1), 64, false);
        let inspect = runner.clone();
        let before_boundary = inspect.boundary_payload.as_ref().unwrap().snapshot();
        let mut config = plane(true, false);
        match invalid {
            "duplicate" => config.program.indexed_metadata_inputs.push("S".into()),
            "unknown" => config
                .program
                .indexed_metadata_inputs
                .push("missing".into()),
            "constant_row" => {
                let ExprSpec::Read { indices, .. } = &mut config.program.assignments[0].expr else {
                    unreachable!()
                };
                indices[0] = IndexSpec::Affine {
                    affine: crate::processing_element::program::syntax::AffineSpec {
                        terms: std::collections::BTreeMap::new(),
                        offset: 0,
                    },
                };
                // A constant row no longer infers n from this first read.
                // Declare the existing eight-row fixture extent explicitly,
                // prove ordinary validity, then retain the indexed rejection.
                config.program.extents.insert("n".into(), 8);
                let mut ordinary = config.program.clone();
                ordinary.indexed_metadata_inputs.clear();
                validate_program(
                    &ordinary,
                    &RegionBindings {
                        inputs: config.inputs.clone(),
                        outputs: config.outputs.clone(),
                    },
                )
                .unwrap();
            }
            "native_scale_dtype" => {
                // Existing native MXFP4 uses Fp8 block scale storage, not Uint8
                // scalar planes. Validate the exact ordinary
                // program first; no malformed MatrixMac oracle.
                config.program = yaml_serde::from_str(
                    r"
inputs: [A, B, SA, SB]
metadata_inputs: [SA, SB]
outputs: [Y]
assignments:
 - output: Y
   axes: [m, n]
   expr:
    matrix_mac:
     execution: {instruction: mxfp4, accumulator: fp32, sparsity: dense}
     over: [k]
     args:
      - {read: {tensor: A, indices: [m, k]}}
      - {read: {tensor: B, indices: [n, k]}}
     quantization:
      - {format: mxfp4, block_size: 32, block_scales: SA, layout: logical}
      - {format: mxfp4, block_size: 32, block_scales: SB, layout: logical}
",
                )
                .unwrap();
                let view = |shape: &[usize], dtype, addr| {
                    TensorView::new_full(Tensor::new(shape, &dtype, addr).unwrap())
                };
                config.inputs = vec![
                    view(&[2, 32], DataType::Fp4, 0),
                    view(&[2, 32], DataType::Fp4, 128),
                    view(&[2, 1], DataType::Fp8, 256),
                    view(&[2, 1], DataType::Fp8, 264),
                ];
                config.outputs = vec![view(&[2, 2], DataType::Fp32, 1024)];
                config.tiling = None;
                validate_program(
                    &config.program,
                    &RegionBindings {
                        inputs: config.inputs.clone(),
                        outputs: config.outputs.clone(),
                    },
                )
                .unwrap();
                config.program.indexed_metadata_inputs.push("SA".into());
            }
            _ => unreachable!(),
        }
        engine
            .spawner()
            .spawn(async move { runner.run_program(0, &config).await });
        let error = engine.run().unwrap_err();
        let expected = match invalid {
            "duplicate" | "unknown" => "distinct subset",
            "constant_row" => "[output axis, constant affine column]",
            "native_scale_dtype" => "rank-two Uint8 plane",
            _ => unreachable!(),
        };
        assert!(error.to_string().contains(expected), "{invalid}: {error}");
        println!("INDEXED_SCALE_REJECT {invalid} {error}");
        assert_eq!(done.0.get(), 0);
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
        let stats = inspect.stats.borrow();
        assert_eq!(
            (stats.read_bytes, stats.written_bytes, stats.peak_sram_bytes),
            (0, 0, 0)
        );
        assert_eq!(
            stats.machine_ops,
            crate::processing_element::MachineOpCounts::default()
        );
        assert_eq!(inspect.sram.used(), 0);
        assert_eq!(
            inspect.boundary_payload.as_ref().unwrap().snapshot(),
            before_boundary
        );
        for lanes in [
            &inspect.activity_lanes.lsu_read,
            &inspect.activity_lanes.lsu_write,
            &inspect.activity_lanes.compute_wait,
        ] {
            assert_eq!(lanes.borrow().active_count_for_test(), 0);
            assert_eq!(lanes.borrow().allocated_count_for_test(), 0);
        }
        for lanes in &inspect.activity_lanes.compute {
            assert_eq!(lanes.borrow().allocated_count_for_test(), 0);
        }
    }
}
