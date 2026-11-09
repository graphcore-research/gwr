// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Unexecuted source scalar runner probes; expected counters are independent
//! source arithmetic.
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
fn fixture(
    engine: &mut gwr_engine::engine::Engine,
    rate: Option<u64>,
    capacity: usize,
    cached: bool,
) -> (
    TaskRunner,
    Rc<Memory<MemoryAccess>>,
    Rc<Completed>,
    Option<Rc<Cache<MemoryAccess>>>,
) {
    fixture_named(engine, rate, capacity, cached, "pe", "hbm")
}
fn fixture_named(
    engine: &mut gwr_engine::engine::Engine,
    rate: Option<u64>,
    capacity: usize,
    cached: bool,
    pe_name: &str,
    memory_name: &str,
) -> (
    TaskRunner,
    Rc<Memory<MemoryAccess>>,
    Rc<Completed>,
    Option<Rc<Cache<MemoryAccess>>>,
) {
    fixture_with_request_gate(engine, rate, capacity, cached, pe_name, memory_name, None)
}

#[expect(clippy::too_many_arguments)]
fn fixture_with_request_gate(
    engine: &mut gwr_engine::engine::Engine,
    rate: Option<u64>,
    capacity: usize,
    cached: bool,
    pe_name: &str,
    memory_name: &str,
    gate: Option<(Rc<Cell<bool>>, Rc<Cell<bool>>)>,
) -> (
    TaskRunner,
    Rc<Memory<MemoryAccess>>,
    Rc<Completed>,
    Option<Rc<Cache<MemoryAccess>>>,
) {
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
        boundary_payload_service: None,
        local_payload_service: rate,
        compute: yaml_serde::from_str(
            "engines: {alu: {capabilities: [{class: binary, ops_per_tick: 1}, {class: unary, ops_per_tick: 1}, {class: ternary, ops_per_tick: 1}]}}",
        )
        .unwrap(),
        operation_classes: yaml_serde::from_str("binary: {operations: [add, sub, shift_right, shift_left, bitand, not_equal], arity: 2}\nunary: {operations: [convert, reinterpret], arity: 1}\nternary: {operations: [select], arity: 3}").unwrap(),
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
        if let Some((observed, release)) = gate {
            use gwr_engine::port::{InPort, OutPort};
            use gwr_engine::traits::Routable;
            let mut rx =
                InPort::<MemoryAccess>::new(engine, &clock, engine.top(), "selected_input_gate_rx");
            let mut tx = OutPort::<MemoryAccess>::new(engine.top(), "selected_input_gate_tx");
            pe.connect_port_tx(rx.state()).unwrap();
            tx.connect(memory.port_rx()).unwrap();
            let gate_clock = clock.clone();
            engine.spawner().spawn(async move {
                let mut held_once = false;
                loop {
                    let access = rx.start_get()?.await;
                    if !held_once && access.dst_addr() == 128 {
                        held_once = true;
                        observed.set(true);
                        while !release.get() {
                            gate_clock.wait_ticks(1).unwrap().await;
                        }
                    }
                    // Preserve the actual original packet and intake ACK.
                    tx.put(access)?.await;
                    rx.finish_get();
                }
            });
        } else {
            pe.connect_port_tx(memory.port_rx()).unwrap();
        }
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

fn round(enabled: bool) -> ProgramTaskConfig {
    let mut program: ProgramSpec =
        yaml_serde::from_str(include_str!("../v41_round_scale_scalar.json")).unwrap();
    program.local_round_policy = enabled.then_some(
        crate::processing_element::program::syntax::LocalRoundPolicy::V41RoundScaleScalar,
    );
    let types = [
        DataType::Fp32,
        DataType::Uint32,
        DataType::Uint32,
        DataType::Uint32,
        DataType::Uint32,
        DataType::Int32,
        DataType::Int32,
        DataType::Int32,
        DataType::Int32,
    ];
    let inputs = types
        .iter()
        .enumerate()
        .map(|(i, dtype)| {
            let shape = if i == 0 { vec![1] } else { vec![] };
            TensorView::new_full(Tensor::new(&shape, dtype, (i * 16) as u64).unwrap())
        })
        .collect();
    ProgramTaskConfig {
        pipeline: None,
        id: "round".into(),
        program,
        tiling: None,
        inputs,
        outputs: vec![TensorView::new_full(
            Tensor::new(&[1], &DataType::Fp32, 1024).unwrap(),
        )],
    }
}

#[test]
fn independent_scalar_table_matches_actual_slots_and_logical_byte_footprints() {
    let cfg = round(true);
    crate::processing_element::local_payload::preflight(&cfg, true, 112).unwrap();
    let region = validate_program(
        &cfg.program,
        &RegionBindings {
            inputs: cfg.inputs.clone(),
            outputs: cfg.outputs.clone(),
        },
    )
    .unwrap();
    let tile = plan_tiles(&region, None).unwrap().tiles().next().unwrap();
    let allocation =
        crate::processing_element::program::plan::allocate_temporaries(&region, &tile).unwrap();
    let names = allocation
        .slots
        .iter()
        .map(|s| s.values.iter().map(String::as_str).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            vec!["Bits", "MaskedExp", "Exponent", "Rebiased"],
            vec!["ExpBits", "ExpI", "Ceil", "Assembled"],
            vec!["Mantissa", "Increment"],
            vec!["Nonzero"]
        ]
    );
    assert_eq!(
        allocation
            .slots
            .iter()
            .map(|s| s.capacity_bytes)
            .collect::<Vec<_>>(),
        vec![8, 8, 8, 8]
    );
    assert_eq!(allocation.arena_bytes, 32);
    assert_eq!(
        crate::processing_element::local_payload::boundary_arena_bytes(&cfg).unwrap(),
        80
    );
    let f = crate::processing_element::local_payload::footprints(&region, &tile, &allocation, 80)
        .unwrap();
    assert_eq!(
        f.iter()
            .map(|v| (v.read_bytes, v.write_bytes))
            .collect::<Vec<_>>(),
        vec![
            (0, 4),
            (4, 4),
            (4, 4),
            (4, 4),
            (4, 4),
            (4, 4),
            (4, 1),
            (1, 4),
            (8, 4),
            (4, 4),
            (4, 4),
            (4, 0)
        ]
    );
    assert_eq!(f[6].write_ranges, vec![104..105]);
    assert_eq!(f[8].read_ranges, vec![80..84, 96..100]);
    assert_eq!(f[11].read_ranges, vec![88..92]);
}

fn execute(
    rate: Option<u64>,
    enabled: bool,
) -> (
    u64,
    super::super::local_payload::LocalPayloadSnapshot,
    usize,
    usize,
    usize,
    usize,
    crate::processing_element::MachineOpCounts,
) {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done, _) = fixture(&mut engine, rate, 112, false);
    let inspect = runner.clone();
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &round(enabled)).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    assert_eq!(inspect.sram.used(), 0);
    let snapshot = inspect
        .local_payload
        .as_ref()
        .map_or(Default::default(), |s| s.snapshot().into());
    let stats = inspect.stats.borrow();
    (
        engine.default_clock().tick_now().tick(),
        snapshot,
        stats.read_bytes,
        stats.written_bytes,
        memory.bytes_read(),
        memory.bytes_written(),
        stats.machine_ops.clone(),
    )
}
#[test]
fn actual_round_local_counters_rate_effect_and_unchanged_external_work() {
    let plain = execute(None, false);
    let off = execute(Some(1), false);
    assert_eq!(
        plain, off,
        "PE service alone must not change any None behavior"
    );
    let fast = execute(Some(8), true);
    let slow = execute(Some(1), true);
    for result in [&fast, &slow] {
        assert_eq!(
            (
                result.1.admitted_read_bytes,
                result.1.completed_read_bytes,
                result.1.admitted_write_bytes,
                result.1.completed_write_bytes
            ),
            (45, 45, 41, 41)
        );
        assert_eq!(
            (result.1.canceled_read_bytes, result.1.canceled_write_bytes),
            (0, 0)
        );
        assert_eq!((result.2, result.3, result.4, result.5), (36, 4, 576, 64));
        assert_eq!(result.6, plain.6);
    }
    assert!(slow.0 > fast.0);
    assert!(fast.0 > plain.0);
    println!("LOCAL_ROUND_NATIVE plain={plain:?} fast={fast:?} slow={slow:?}");
}

#[test]
fn read_before_arithmetic_and_write_before_dependency_ready_are_real_milestones() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, _, done, _) = fixture(&mut engine, Some(1), 112, false);
    let inspect = runner.clone();
    let clock = engine.default_clock();
    let work = runner.clone();
    engine
        .spawner()
        .spawn(async move { work.run_program(0, &round(true)).await });
    engine.spawner().spawn(async move {
        // Observe actual phase counters rather than guess absolute input
        // completion.
        for attempt in 0..4096 {
            let s = inspect.local_payload.as_ref().unwrap().snapshot();
            assert!(
                attempt < 4095,
                "expected actual local milestone was not observed"
            );
            if s.admitted_write_bytes >= 4 && s.completed_write_bytes == 0 {
                break;
            }
            clock.wait_ticks(1).unwrap().await;
        }
        assert_eq!(
            inspect
                .stats
                .borrow()
                .machine_ops
                .checked_operation_total()
                .unwrap(),
            1,
            "first reinterpret completed but its output write is pending"
        );
        assert_eq!(
            inspect
                .local_payload
                .as_ref()
                .unwrap()
                .snapshot()
                .admitted_read_bytes,
            0,
            "dependent ExpBits read cannot precede Bits write"
        );
        for attempt in 0..4096 {
            let s = inspect.local_payload.as_ref().unwrap().snapshot();
            assert!(
                attempt < 4095,
                "expected actual local milestone was not observed"
            );
            if s.admitted_read_bytes >= 4 && s.completed_read_bytes == 0 {
                break;
            }
            clock.wait_ticks(1).unwrap().await;
        }
        assert_eq!(
            inspect
                .stats
                .borrow()
                .machine_ops
                .checked_operation_total()
                .unwrap(),
            1,
            "ExpBits arithmetic must wait for actual first local read completion"
        );
        assert_eq!(inspect.sram.used(), 112);
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
}

#[test]
fn canceled_local_read_retains_debt_without_completion_then_same_runner_retries() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, done, _) = fixture(&mut engine, Some(1), 112, false);
    let inspect = runner.clone();
    let clock = engine.default_clock();
    let retry = runner.clone();
    let drain_memory = memory.clone();
    let pending_done = done.clone();
    engine.spawner().spawn(async move {
        let config = round(true);
        // Independent literal source table: the final Y reinterpret consumes
        // Assembled only after both source branches and every input have joined.
        let region = validate_program(&config.program, &RegionBindings {
            inputs: config.inputs.clone(), outputs: config.outputs.clone(),
        }).unwrap();
        let tile = plan_tiles(&region, None).unwrap().tiles().next().unwrap();
        let allocation = crate::processing_element::program::plan::allocate_temporaries(&region, &tile).unwrap();
        let source_table = crate::processing_element::local_payload::footprints(&region, &tile, &allocation, 80).unwrap();
        assert_eq!(source_table.iter().map(|f| (f.read_bytes, f.write_bytes)).collect::<Vec<_>>(),
            vec![(0,4),(4,4),(4,4),(4,4),(4,4),(4,4),(4,1),(1,4),(8,4),(4,4),(4,4),(4,0)]);
        assert_eq!(source_table[..11].iter().map(|f| f.read_bytes).sum::<usize>(), 41);
        assert_eq!(source_table[..11].iter().map(|f| f.write_bytes).sum::<usize>(), 41);
        assert_eq!(config.program.assignments.len(), 12);
        assert_eq!(config.program.assignments[11].primary_result(), "Y");
        {
            let work = runner.run_program(0, &config);
            futures::pin_mut!(work);
            let chosen = async {
                for attempt in 0..4096 {
                    let s = retry.local_payload.as_ref().unwrap().snapshot();
                    assert!(
                        attempt < 4095,
                        "expected cancellable local read phase was not observed"
                    );
                    // Select the final Y read, not the disproved early-read
                    // overlap window. All original boundary replies must have
                    // settled before logical Drop releases the actual arena.
                    if s.admitted_read_bytes == 45
                        && s.completed_read_bytes == 41
                        && s.completed_write_bytes == 41
                        && drain_memory.bytes_read() == 576
                        && retry.stats.borrow().read_bytes == 36
                    {
                        assert_eq!(retry.sram.used(), 112);
                        assert_eq!(pending_done.0.get(), 0);
                        assert_eq!(drain_memory.bytes_written(), 0);
                        assert_eq!(retry.stats.borrow().written_bytes, 0);
                        println!("LOCAL_CANCEL_FINAL_Y_DRAINED_INPUTS tick={} local={s:?} memory_read={} pe_read={}",
                            clock.tick_now().tick(), drain_memory.bytes_read(), retry.stats.borrow().read_bytes);
                        break;
                    }
                    clock.wait_ticks(1).unwrap().await;
                }
            };
            futures::pin_mut!(chosen);
            match select(work, chosen).await {
                Either::Right((_, pending)) => drop(pending),
                Either::Left(_) => panic!("selected local read must be pending"),
            };
        }
        assert_eq!(
            retry.sram.used(),
            0,
            "no local byte worker remains after logical Drop"
        );
        assert_eq!(
            drain_memory.bytes_read(),
            576,
            "all external input replies completed before selected logical cancellation"
        );
        assert_eq!(
            retry.stats.borrow().read_bytes,
            36,
            "PE has consumed all actual input replies"
        );
        let canceled = retry.local_payload.as_ref().unwrap().snapshot();
        assert_eq!(
            (canceled.canceled_read_bytes, canceled.completed_read_bytes),
            (4, 41)
        );
        assert_eq!(
            retry
                .stats
                .borrow()
                .machine_ops
                .checked_operation_total()
                .unwrap(),
            11,
            "the final Y reinterpret cannot execute after canceled read"
        );
        retry.run_program(1, &config).await
    });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    let s = inspect.local_payload.as_ref().unwrap().snapshot();
    assert_eq!(
        (
            s.admitted_read_bytes,
            s.completed_read_bytes,
            s.canceled_read_bytes
        ),
        (90, 86, 4)
    );
    assert_eq!((s.admitted_write_bytes, s.completed_write_bytes), (82, 82));
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (1152, 64));
    assert_eq!(inspect.sram.used(), 0);
}

#[test]
fn two_same_pe_source_tasks_share_one_budget_while_two_pes_are_independent() {
    let mut same = gwr_engine::engine::Engine::default();
    let (a, _, done, _) = fixture(&mut same, Some(1), 224, false);
    let b = a.clone();
    let inspect = a.clone();
    assert!(Rc::ptr_eq(
        a.local_payload.as_ref().unwrap(),
        b.local_payload.as_ref().unwrap()
    ));
    same.spawner()
        .spawn(async move { a.run_program(0, &round(true)).await });
    same.spawner()
        .spawn(async move { b.run_program(1, &round(true)).await });
    same.run().unwrap();
    assert_eq!(done.0.get(), 2);
    let shared = same.default_clock().tick_now().tick();
    assert_eq!(
        (
            inspect
                .local_payload
                .as_ref()
                .unwrap()
                .snapshot()
                .completed_read_bytes,
            inspect
                .local_payload
                .as_ref()
                .unwrap()
                .snapshot()
                .completed_write_bytes
        ),
        (90, 82)
    );
    let mut separate = gwr_engine::engine::Engine::default();
    let (a, _, da, _) = fixture_named(&mut separate, Some(1), 112, false, "a", "ha");
    let (b, _, db, _) = fixture_named(&mut separate, Some(1), 112, false, "b", "hb");
    assert!(!Rc::ptr_eq(
        a.local_payload.as_ref().unwrap(),
        b.local_payload.as_ref().unwrap()
    ));
    separate
        .spawner()
        .spawn(async move { a.run_program(0, &round(true)).await });
    separate
        .spawner()
        .spawn(async move { b.run_program(1, &round(true)).await });
    separate.run().unwrap();
    assert_eq!((da.0.get(), db.0.get()), (1, 1));
    assert!(shared > separate.default_clock().tick_now().tick());
}

#[test]
fn happy_first_unsupported_frontier_rejects_before_any_runner_effects() {
    let valid = round(true);
    crate::processing_element::local_payload::preflight(&valid, true, 112).unwrap();
    let mut bad = vec![];
    let mut ast = valid.clone();
    ast.program.assignments[1].axes = vec![];
    bad.push(ast);
    let mut typed = valid.clone();
    typed.inputs[0] = TensorView::new_full(Tensor::new(&[1], &DataType::Bf16, 0).unwrap());
    bad.push(typed);
    let mut batch = valid.clone();
    batch.inputs[0] = TensorView::new_full(Tensor::new(&[2], &DataType::Fp32, 0).unwrap());
    bad.push(batch);
    let mut metadata = valid.clone();
    metadata.program.metadata_inputs.push("A".into());
    bad.push(metadata);
    let mut tiled = valid.clone();
    tiled.tiling = Some(yaml_serde::from_str("sizes: {b: 1}").unwrap());
    bad.push(tiled);
    for cfg in bad {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, done, _) = fixture(&mut engine, Some(1), 112, false);
        let inspect = runner.clone();
        engine.spawner().spawn(async move {
            let error = runner.run_program(0, &cfg).await.unwrap_err();
            assert!(error.0.contains("local round policy"), "{error}");
            Ok(())
        });
        engine.run().unwrap();
        assert_eq!(done.0.get(), 0);
        assert_eq!(inspect.sram.used(), 0);
        assert_eq!(inspect.stats.borrow().peak_sram_bytes, 0);
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
            inspect.local_payload.as_ref().unwrap().snapshot(),
            Default::default()
        );
        assert_eq!(
            inspect
                .activity_lanes
                .local_payload
                .borrow()
                .active_count_for_test(),
            0
        );
    }
    assert!(
        crate::processing_element::local_payload::preflight(&valid, false, 112)
            .unwrap_err()
            .0
            .contains("service")
    );
    assert!(
        crate::processing_element::local_payload::preflight(&valid, true, 111)
            .unwrap_err()
            .0
            .contains("SRAM")
    );
}

#[test]
fn returned_local_child_error_holds_arena_until_actual_sibling_request_drains() {
    // Establish the same real source/carrier/service baseline before fault
    // setup.
    let happy = execute(Some(4), true);
    assert_eq!(
        (happy.1.completed_read_bytes, happy.1.completed_write_bytes),
        (45, 41)
    );
    let mut engine = gwr_engine::engine::Engine::default();
    let observed = Rc::new(Cell::new(false));
    let release = Rc::new(Cell::new(false));
    let (runner, memory, done, _) = fixture_with_request_gate(
        &mut engine,
        Some(4),
        112,
        false,
        "pe",
        "hbm",
        Some((observed.clone(), release.clone())),
    );
    let inspect = runner.clone();
    let clock = engine.default_clock();
    // Public checked service API deliberately exhausts the write counter.
    // Cancellation retains debt. The first real Bits compute succeeds, then
    // its four-byte local write admission fails without publishing readiness.
    // Rate4 keeps the accepted seed deadline representable on this64bit target.
    let seed = runner
        .local_payload
        .as_ref()
        .unwrap()
        .reserve(
            crate::processing_element::local_payload::LocalPayloadDirection::Write,
            usize::MAX,
        )
        .unwrap();
    drop(seed);
    let returned = Rc::new(Cell::new(false));
    let returned_work = returned.clone();
    let inspect_work = runner.clone();
    let done_work = done.clone();
    engine.spawner().spawn(async move {
        let error = runner.run_program(0, &round(true)).await.unwrap_err();
        assert!(
            error.0.contains("admitted payload counter overflows"),
            "actual selected child error: {error}"
        );
        returned_work.set(true);
        assert_eq!(
            inspect_work.sram.used(),
            0,
            "release only after issued child drain"
        );
        assert_eq!(inspect_work.stats.borrow().read_bytes, 36);
        assert_eq!(
            done_work.0.get(),
            0,
            "failed task never publishes dispatch completion"
        );
        assert_eq!(
            inspect_work
                .stats
                .borrow()
                .machine_ops
                .checked_operation_total()
                .unwrap(),
            1,
            "failed local write never publishes Bits readiness to its consumers"
        );
        // Seeded counter exhaustion cannot be repaired by pretending debt was
        // cleared. The exact same runner safely retries ordinary None after
        // all actual ports/arena ownership drain; no service state is reset.
        inspect_work.run_program(1, &round(false)).await
    });
    let watch = inspect.clone();
    let returned_watch = returned.clone();
    let memory_watch = memory.clone();
    engine.spawner().spawn(async move {
        for attempt in 0..4096 {
            assert!(
                attempt < 4095,
                "actual held sibling/first compute not reached"
            );
            if observed.get()
                && watch
                    .stats
                    .borrow()
                    .machine_ops
                    .checked_operation_total()
                    .unwrap()
                    == 1
                && watch.stats.borrow().read_bytes == 32
            {
                break;
            }
            clock.wait_ticks(1).unwrap().await;
        }
        assert_eq!(
            memory_watch.bytes_read(),
            512,
            "selected fourth-byte input still held before actual Memory intake"
        );
        assert_eq!(watch.stats.borrow().read_bytes, 32);
        for _ in 0..16 {
            assert!(
                !returned_watch.get(),
                "cannot return while issued sibling has no actual reply"
            );
            assert_eq!(watch.sram.used(), 112);
            assert_eq!(done.0.get(), 0);
            assert_eq!(
                watch
                    .local_payload
                    .as_ref()
                    .unwrap()
                    .snapshot()
                    .completed_write_bytes,
                0
            );
            assert_eq!(
                watch
                    .stats
                    .borrow()
                    .machine_ops
                    .checked_operation_total()
                    .unwrap(),
                1
            );
            clock.wait_ticks(1).unwrap().await;
        }
        release.set(true);
        Ok(())
    });
    engine.run().unwrap();
    assert!(returned.get());
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (1152, 64));
    assert_eq!(
        inspect
            .stats
            .borrow()
            .machine_ops
            .checked_operation_total()
            .unwrap(),
        13
    );
    assert_eq!(inspect.sram.used(), 0);
}
