// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Source-only independent address/lifetime/native hypotheses.
use super::*;
use crate::processing_element::compute::{ExecutionMode, Sparsity};
use crate::processing_element::tensors::Tensor;
fn config(
    m: usize,
    n: usize,
    k: usize,
    split: usize,
    p: usize,
    no: usize,
) -> StreamedMatrixTaskConfig {
    let view = |shape: &[usize], base| {
        TensorView::new_full(Tensor::new(shape, &DataType::Bf16, base).unwrap())
    };
    StreamedMatrixTaskConfig {
        id: "source".into(),
        a: view(&[m, k], 0),
        b: view(&[n, k], 65536),
        c: view(&[m, n], 40_000_000),
        invocation: 9,
        generation: 1,
        m,
        n,
        k,
        split,
        participant: p,
        n_offset: no,
        execution: ExecutionMode {
            instruction: "tensor".into(),
            accumulator: DataType::Fp32,
            sparsity: Sparsity::Dense,
        },
    }
}
#[test]
fn independent_source_addresses_retain_padded_duplicates_and_b_order() {
    for (m, n, k, split) in [
        (1, 64, 512, 2),
        (1, 7168, 2048, 4),
        (2, 7168, 2048, 4),
        (4, 7168, 2048, 4),
    ] {
        for p in 0..split {
            let c = config(m, n, k, split, p, 0);
            c.validate().unwrap();
            assert_eq!(c.chunk_count(), if split == 2 { 2 } else { 4 });
            for chunk in 0..c.chunk_count() {
                let observed = c.issues(chunk).unwrap().collect::<Vec<_>>();
                assert_eq!(observed.len(), 1536);
                // Independent tuple loops reproduce source register/thread
                // enumeration, explicit XOR byte permutation
                // and original safe-row-zero branch.
                let mut expected = Vec::new();
                for (operand, rows, base) in
                    [(SourceOperand::B, 64, 65536_u64), (SourceOperand::A, 32, 0)]
                {
                    for reg in 0..rows / 8 {
                        for tid in 0..128 {
                            let row = (reg * 128 + tid) / 16;
                            let vector = (reg * 128 + tid) % 16;
                            let col = ((vector ^ (row % 16)) * 8) + p * (k / split) + chunk * 128;
                            let actual_row = if operand == SourceOperand::A && row >= m {
                                0
                            } else {
                                row
                            };
                            expected.push((operand, base + ((actual_row * k + col) * 2) as u64));
                        }
                    }
                }
                assert_eq!(
                    observed
                        .iter()
                        .map(|v| (v.operand, v.address))
                        .collect::<Vec<_>>(),
                    expected
                );
                let a = observed
                    .iter()
                    .filter(|v| v.operand == SourceOperand::A)
                    .map(|v| v.address)
                    .collect::<Vec<_>>();
                assert_eq!(a.len(), 512);
                assert!(
                    a.iter()
                        .copied()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        < a.len()
                );
            }
        }
    }
    let c = config(1, 7168, 2048, 4, 3, 64);
    assert_eq!(
        c.issues(0).unwrap().next().unwrap().address,
        65536 + ((64 * 2048 + 1536) * 2) as u64
    );
}
#[test]
fn valid_first_source_shape_dtype_execution_and_pair_mutations_reject() {
    let c = config(1, 64, 512, 2, 0, 0);
    c.validate().unwrap();
    let mut wrong = c.clone();
    wrong.k = 256;
    assert!(wrong.validate().is_err());
    let mut wrong = c.clone();
    wrong.participant = 2;
    assert!(wrong.validate().is_err());
    let mut wrong = c.clone();
    wrong.execution.accumulator = DataType::Bf16;
    assert!(wrong.validate().is_err());
    let mut wrong = c.clone();
    wrong.n_offset = 1;
    assert!(wrong.validate().is_err());
    let slot = StreamedOutputSlot::new(c.clone()).unwrap();
    let effect = crate::memory::atomic::Effect::Atomic {
        participant: 0,
        word: crate::memory::atomic::AtomicWord {
            device: 0,
            address: c.c.tensor().addr(),
            invocation: 9,
            generation: 1,
            operation: crate::memory::atomic::AtomicOperation::Bf16PairFadd,
            scope: crate::memory::atomic::AtomicScope::Agent,
            ordering: crate::memory::atomic::AtomicOrdering::Monotonic,
        },
    };
    slot.validate_word(effect, 0).unwrap();
    assert!(slot.take(effect, 0).is_err());
    assert!(slot.validate_word(effect, 1).is_err());
    assert!(slot.validate_word(effect, 128).is_err());
}

use std::cell::Cell;

use async_trait::async_trait;

use super::super::{Dispatcher, TaskRunner};
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::MemoryMap;
use crate::memory::{Memory, MemoryConfig};
use crate::processing_element::task::Task;
use crate::processing_element::{ProcessingElement, ProcessingElementConfig};
struct Completed(Cell<usize>, RefCell<Option<Rc<StreamedOutputSlot>>>);
#[async_trait(?Send)]
impl crate::processing_element::dispatch::Dispatch for Completed {
    fn task_by_id(&self, _: usize) -> Result<Task, SimError> {
        Ok(crate::processing_element::task::Task::StreamedMatrix {
            config: self.1.borrow().as_ref().unwrap().clone(),
        })
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
fn fixture_with_map(
    engine: &mut gwr_engine::engine::Engine,
    capacity: Option<u64>,
    replicated: bool,
    boundary: Option<u64>,
    sram_bytes: usize,
    custom_map: Option<MemoryMap>,
) -> (
    TaskRunner,
    Rc<Memory<MemoryAccess>>,
    Rc<Completed>,
    Rc<ProcessingElement>,
) {
    let clock = engine.default_clock();
    let map = custom_map.unwrap_or_else(|| {
        let mut map = MemoryMap::new();
        map.insert(0, 67_108_864, gwr_engine::types::DeviceId(0))
            .unwrap();
        map
    });
    let map = Rc::new(map);
    let policy = "";
    let budget = capacity
        .map(|v| format!("matrix_accumulator_capacity_bytes: {v}, "))
        .unwrap_or_default();
    let engine_cfg = format!(
        "{budget}capabilities: [{{class: matrix,input_dtypes: [[bf16,bf16]],ops_per_tick: 1000,execution: {{instruction: tensor,accumulator: fp32,sparsity: dense}}{policy}}}]"
    );
    let compute = if replicated {
        format!("engines: {{matrix0: {{{engine_cfg}}}, matrix1: {{{engine_cfg}}}}}")
    } else {
        format!(
            "engines: {{matrix0: {{{engine_cfg}}},alu: {{capabilities: [{{class: convert,input_dtypes: [[fp32]],ops_per_tick: 1000}}]}}}}"
        )
    };
    let cfg = ProcessingElementConfig {
        block_execution: None,
        num_active_requests: 16,
        lsu_access_bytes: 16,
        overhead_size_bytes: 0,
        sram_bytes,
        boundary_payload_service: boundary,
        local_payload_service: None,
        compute: yaml_serde::from_str(&compute).unwrap(),
        operation_classes: yaml_serde::from_str("matrix: {operations: [matrix_mac],arity: 2}\nconvert: {operations: [convert],arity: 1}")
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
        MemoryConfig::new(0, 67_108_864, 65536, 0)
            .with_arrival_aware_service(16)
            .with_service_lanes(4, vec![65536])
            .with_atomic_service(
                0,
                crate::memory::atomic::AtomicServiceConfig {
                    capacity: 16,
                    bytes_per_tick: 65536,
                    delay_ticks: 0,
                    bf16_pair_per_tick: Some(1000),
                    i32_fetch_add_per_tick: Some(1000),
                    lane_interleave_bytes: 4,
                    lane_bytes_per_tick: vec![65536],
                },
            ),
    )
    .unwrap();
    pe.connect_port_tx(memory.port_rx()).unwrap();
    memory.connect_port_tx(pe.port_rx()).unwrap();
    let done = Rc::new(Completed(Cell::new(0), RefCell::new(None)));
    let dispatcher: Dispatcher = done.clone();
    // The registered PE has no scheduled tasks; direct runner controls own the
    // work.
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
    (runner, memory, done, pe)
}

fn fixture(
    engine: &mut gwr_engine::engine::Engine,
    capacity: Option<u64>,
    replicated: bool,
    boundary: Option<u64>,
    sram_bytes: usize,
) -> (
    TaskRunner,
    Rc<Memory<MemoryAccess>>,
    Rc<Completed>,
    Rc<ProcessingElement>,
) {
    fixture_with_map(engine, capacity, replicated, boundary, sram_bytes, None)
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep complete setup execution and assertions of this scenario together"
)]
fn actual_source_stream_dispatch_scratch_survives_last_real_atomic_ack_and_cancel_drain() {
    for abandon_last in [false, true] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, done, pe) = fixture(&mut engine, Some(8192), false, None, 49152);
        let c = config(1, 64, 512, 2, 0, 0);
        let slot =
            StreamedOutputSlot::new_bound(c.clone(), &pe, memory.atomic_target().unwrap()).unwrap();
        *done.1.borrow_mut() = Some(slot.clone());
        let inspect = runner.clone();
        let after = runner.clone();
        let actual_slot = slot.clone();
        let target = memory.atomic_target().unwrap();
        let ctl = memory.atomic_control().unwrap();
        let observe = ctl.clone();
        let clock = engine.default_clock();
        let during = runner.clone();
        let probe = clock.clone();
        engine.spawner().spawn(async move {
            probe.wait_ticks(3)?.await;
            let (retained, _) = during.compute_scheduler.retained_state_for_test();
            assert_eq!(retained.iter().sum::<u64>(), 8192);
            let matrix = crate::processing_element::compute::EngineId(1);
            let mut contender = during.compute_scheduler.retain_accumulator(matrix, 8192);
            assert!(futures::poll!(&mut contender).is_pending());
            drop(contender);
            let execution = during.compute_scheduler.acquire(matrix).await?;
            drop(execution);
            assert_eq!(
                during
                    .compute_scheduler
                    .retained_state_for_test()
                    .0
                    .iter()
                    .sum::<u64>(),
                8192
            );
            Ok(())
        });
        engine.spawner().spawn(async move {
            runner.run(0).await?;
            assert_eq!(done.0.get(), 1);
            assert_eq!(inspect.sram.used(), 49152);
            assert_eq!(
                inspect.compute_scheduler.retained_state_for_test().0,
                vec![0, 0]
            );
            for pair in 0..32 {
                let offset = pair * 4;
                let effect = crate::memory::atomic::Effect::Atomic {
                    participant: 0,
                    word: crate::memory::atomic::AtomicWord {
                        device: 0,
                        address: c.c.tensor().addr() + offset as u64,
                        invocation: 9,
                        generation: 1,
                        operation: crate::memory::atomic::AtomicOperation::Bf16PairFadd,
                        scope: crate::memory::atomic::AtomicScope::Agent,
                        ordering: crate::memory::atomic::AtomicOrdering::Monotonic,
                    },
                };
                let operand = actual_slot.take(effect, offset)?;
                let receipt = target
                    .receive_with_operand(
                        0,
                        c.c.tensor().addr() + offset as u64,
                        effect,
                        Some(operand),
                    )
                    .await?;
                if pair == 31 {
                    assert!(actual_slot.published.borrow().is_none());
                    clock.wait_ticks(3)?.await;
                    assert_eq!(inspect.sram.used(), 49152);
                    assert_eq!(ctl.snapshot().current, 1);
                    if abandon_last {
                        drop(receipt);
                        clock.wait_ticks(1)?.await;
                    } else {
                        receipt.acknowledge().await?;
                    }
                } else {
                    receipt.acknowledge().await?;
                }
            }
            Ok(())
        });
        engine.run().unwrap();
        assert_eq!(after.sram.used(), 0);
        assert_eq!(after.stats.borrow().peak_sram_bytes, 49152);
        assert_eq!(
            after.stats.borrow().machine_ops.by_operation["matrix_mac"],
            524_288
        );
        assert_eq!(
            after.stats.borrow().machine_ops.by_operation["convert"],
            2048
        );
        assert_eq!(after.stats.borrow().read_bytes, 49152);
        assert_eq!(after.stats.borrow().written_bytes, 0);
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (49280, 128));
        let atomic = observe.snapshot();
        assert_eq!(atomic.current, 0);
        assert_eq!(atomic.bf16_pair_requests, 32);
        assert_eq!(
            atomic.canceled_aftercommit,
            usize::from(abandon_last) as u64
        );
        let (retained, peak) = after.compute_scheduler.retained_state_for_test();
        assert_eq!(retained, vec![0, 0]);
        assert_eq!(peak.iter().copied().max(), Some(8192));
    }
}
#[test]
fn happy_first_preflight_rejects_one_byte_below_lds_accumulator_and_unmapped_tail_zero_effects() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, _, _) = fixture(&mut engine, Some(8192), false, None, 49152);
    let c = config(1, 64, 512, 2, 0, 0);
    runner.preflight_stream(&c).unwrap();
    assert_eq!(memory.bytes_read(), 0);
    for (capacity, sram) in [(8191, 49152), (8192, 49151)] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, _, _) = fixture(&mut engine, Some(capacity), false, None, sram);
        assert!(runner.preflight_stream(&c).is_err());
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
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
    let mut wrong = c;
    wrong.a = TensorView::new_full(Tensor::new(&[1, 512], &DataType::Bf16, 67_108_864).unwrap());
    assert!(runner.preflight_stream(&wrong).is_err());
    assert_eq!(memory.bytes_read(), 0);
}

#[test]
fn actual_partial_source_cancellation_drains_and_accounts_each_completed_reply_before_retry() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, _, _) = fixture(&mut engine, Some(8192), false, None, 49152);
    let c = config(1, 64, 512, 2, 0, 0);
    let during = runner.clone();
    let physical = memory.clone();
    let clock = engine.default_clock();
    let after = runner.clone();
    let canceled_bytes = Rc::new(Cell::new(0));
    let retained = canceled_bytes.clone();
    engine.spawner().spawn(async move {
        let pending = Box::pin(runner.run_stream(&c));
        let stop = Box::pin(clock.wait_ticks(3)?);
        match futures::future::select(pending, stop).await {
            futures::future::Either::Right(((), pending)) => drop(pending),
            futures::future::Either::Left(_) => {
                panic!("full original source chunk cannot complete by cancellation point")
            }
        }
        assert!(physical.bytes_read() > 0 && physical.bytes_read() < 49152); // reached real partial load, not setup
        // Existing issued source requests remain actor-owned and their LDS
        // backing is retained. Wait for actual source reply slots to
        // drain, never release slots or backing with a fake
        // completion/ACK.
        while during.lsu.source_slots_for_test().iter().any(|busy| *busy) {
            clock.wait_ticks(1)?.await;
        }
        assert_eq!(during.sram.used(), 0);
        let completed = physical.bytes_read();
        assert!(completed > 0 && completed < 49152);
        assert_eq!(during.stats.borrow().read_bytes, completed);
        assert_eq!(
            during
                .stats
                .borrow()
                .machine_ops
                .checked_operation_total()?,
            0
        );
        retained.set(completed);
        let scratch = runner.run_stream(&c).await?;
        drop(scratch);
        assert_eq!(runner.stats.borrow().read_bytes, completed + 49152);
        assert_eq!(physical.bytes_read(), completed + 49152);
        Ok(())
    });
    engine.run().unwrap();
    assert!(canceled_bytes.get() > 0);
    assert_eq!(after.stats.borrow().read_bytes, memory.bytes_read());
    assert_eq!(after.sram.used(), 0);
    assert!(after.lsu.source_slots_for_test().iter().all(|busy| !*busy));
    assert_eq!(
        after.stats.borrow().machine_ops.by_operation["matrix_mac"],
        524_288
    );
    assert_eq!(
        after.stats.borrow().machine_ops.by_operation["convert"],
        2048
    );
}
#[test]
fn actual_acquired_source_slot_end_phase_drop_is_released_then_whole_retry_completes() {
    for cancel in [false, true] {
        let mut engine = gwr_engine::engine::Engine::default();
        let (runner, memory, _, _) = fixture(&mut engine, Some(8192), false, None, 49152);
        let c = config(1, 64, 512, 2, 0, 0);
        let after = runner.clone();
        let clock = engine.default_clock();
        let physical = memory.clone();
        engine.spawner().spawn(async move {
            assert_eq!(
                clock.tick_now().phase(),
                gwr_engine::time::clock::phase::BEGIN
            );
            let mut pending = Box::pin(runner.run_stream(&c));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            // Actual production wait_phase(END) is the first suspension:
            // allocator and retained storage are already granted,
            // one source slot set in_use, and no internal memory
            // request has yet been spawned/issued.
            assert_eq!(
                runner
                    .lsu
                    .source_slots_for_test()
                    .iter()
                    .filter(|busy| **busy)
                    .count(),
                1
            );
            assert_eq!(physical.bytes_read(), 0);
            assert_eq!(runner.stats.borrow().read_bytes, 0);
            assert_eq!(runner.sram.used(), 49152);
            if cancel {
                drop(pending);
                assert!(runner.lsu.source_slots_for_test().iter().all(|busy| !*busy));
                assert_eq!(runner.sram.used(), 0);
                assert_eq!(
                    runner
                        .compute_scheduler
                        .retained_state_for_test()
                        .0
                        .iter()
                        .sum::<u64>(),
                    0
                );
                let scratch = runner.run_stream(&c).await?;
                drop(scratch);
            } else {
                let scratch = pending.await?;
                drop(scratch);
            }
            Ok(())
        });
        engine.run().unwrap();
        assert_eq!(
            (after.stats.borrow().read_bytes, memory.bytes_read()),
            (49152, 49152)
        );
        assert_eq!(after.sram.used(), 0);
        assert!(after.lsu.source_slots_for_test().iter().all(|busy| !*busy));
    }
}
#[test]
fn happy_nonzero_column_block_checks_every_selected_c_word_before_effects() {
    let mut engine = gwr_engine::engine::Engine::default();
    let (runner, memory, _, pe) = fixture(&mut engine, Some(8192), false, None, 49152);
    let c = config(2, 7168, 2048, 4, 0, 64);
    StreamedOutputSlot::new_bound(c.clone(), &pe, memory.atomic_target().unwrap()).unwrap();
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
    assert_eq!(runner.sram.used(), 0);
    for valid_first_row in [false, true] {
        for wrong_device in [false, true] {
            // Either only the base pair is mapped, or every selected pair of
            // row0 is mapped and the later selected row is the
            // first invalid word.
            let end = c.c.tensor().addr()
                + if valid_first_row {
                    2 * (c.n_offset + 64) as u64
                } else {
                    4
                };
            let mut map = MemoryMap::new();
            map.insert(0, end, gwr_engine::types::DeviceId(0)).unwrap();
            if wrong_device {
                map.insert(end, 67_108_864 - end, gwr_engine::types::DeviceId(2))
                    .unwrap();
            }
            let mut engine = gwr_engine::engine::Engine::default();
            let (runner, memory, _, pe) =
                fixture_with_map(&mut engine, Some(8192), false, None, 49152, Some(map));
            // Base C pair is valid; selected n_offset64 block or later M row is
            // not.
            pe.validate_atomic_memory_binding(
                &memory.atomic_target().unwrap(),
                0,
                c.c.tensor().addr(),
            )
            .unwrap();
            let before = memory.atomic_control().unwrap().snapshot();
            let error = match StreamedOutputSlot::new_bound(
                c.clone(),
                &pe,
                memory.atomic_target().unwrap(),
            ) {
                Err(error) => error,
                Ok(_) => panic!("selected C pair must reject before effects"),
            };
            assert!(error.to_string().contains("unmapped or maps"), "{error}");
            assert_eq!(memory.atomic_control().unwrap().snapshot(), before);
            assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
            assert_eq!(
                runner
                    .stats
                    .borrow()
                    .machine_ops
                    .checked_operation_total()
                    .unwrap(),
                0
            );
            assert_eq!(runner.sram.used(), 0);
        }
    }
}
