// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Private native SRAM residency and pure geometry guard controls.
use std::cell::Cell;
use std::collections::BTreeMap;

use gwr_engine::engine::Engine;
use yaml_serde::{from_str, from_value, to_value};

use super::matrix_test_fixture::*;
use super::*;
use crate::memory::Memory;
use crate::memory::memory_access::MemoryAccess;
use crate::processing_element::compute::{MatrixWorkShape, TypedWork};
use crate::processing_element::program::analysis::invocation::AssignmentInvocations;
use crate::processing_element::program::analysis::matrix_shape::build_matrix_work;
use crate::processing_element::program::storage::SRAM_SLOT_ALIGNMENT_BYTES;
use crate::processing_element::tensors::Tensor;
use crate::processing_element::tensors::dtype::DataType;

const SKINNY_M: usize = 1;
const SKINNY_N: usize = MATRIX_SERVICE_M * MATRIX_SERVICE_N;
const SKINNY_K: usize = MATRIX_SERVICE_K;
const SKINNY_MACS: usize = SKINNY_M * SKINNY_N * SKINNY_K;
const PADDED_SKINNY_MACS: usize = SKINNY_M.next_multiple_of(MATRIX_SERVICE_M)
    * SKINNY_N.next_multiple_of(MATRIX_SERVICE_N)
    * SKINNY_K.next_multiple_of(MATRIX_SERVICE_K);
const PLAIN_COMPUTE_TICKS: u64 = SKINNY_MACS.div_ceil(MATRIX_MACS_PER_TICK) as u64;
const PADDED_COMPUTE_TICKS: u64 = PADDED_SKINNY_MACS.div_ceil(MATRIX_MACS_PER_TICK) as u64;
const INPUT_RESPONSE_GAP_TICKS: u64 =
    (MATRIX_OPERAND_COUNT - 1) as u64 * MEMORY_PACKET_SERVICE_TICKS;
const PLAIN_FINISH_TICK: u64 = INPUT_RESPONSE_GAP_TICKS + PLAIN_COMPUTE_TICKS;
const PADDED_FINISH_TICK: u64 = INPUT_RESPONSE_GAP_TICKS + PADDED_COMPUTE_TICKS;
const OUTPUT_BYTES: usize = SKINNY_M * SKINNY_N * BYTES_PER_FP32;
const PEAK_SRAM_BYTES: usize = (SKINNY_M * SKINNY_K * BYTES_PER_BF16)
    .next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES)
    + (SKINNY_K * SKINNY_N * BYTES_PER_BF16).next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES)
    + OUTPUT_BYTES.next_multiple_of(SRAM_SLOT_ALIGNMENT_BYTES);

const ASYMMETRIC_M: usize = 2;
const ASYMMETRIC_N: usize = 3;
const ASYMMETRIC_K: usize = 5;
const ASYMMETRIC_SERVICE_M: usize = ASYMMETRIC_M;
const ASYMMETRIC_SERVICE_N: usize = ASYMMETRIC_N + 1;
const ASYMMETRIC_SERVICE_K: usize = 1;

fn run_service_residency(matrix_service_tiles_enabled: bool) -> u64 {
    let mut engine = Engine::default();
    let (runner, memory, done) =
        runner_with_matrix_service_tiles(&mut engine, matrix_service_tiles_enabled, false);
    let inspect = runner.clone();
    let final_inspect = runner.clone();
    let end = Rc::new(Cell::new(0));
    let observed_end = end.clone();
    let clock = engine.default_clock();
    let finish_clock = clock.clone();
    engine.spawner().spawn(async move {
        runner.run_program(0, &skinny_matrix_program_task()).await?;
        observed_end.set(finish_clock.tick_now().tick());
        Ok(())
    });
    engine.spawner().spawn(async move {
        // Inspect between the plain and padded completion times: only the
        // padded computation should still hold its input/output SRAM.
        let inspect_tick = PLAIN_FINISH_TICK.midpoint(PADDED_FINISH_TICK);
        clock.wait_ticks(inspect_tick)?.await;
        assert_eq!(
            inspect.sram.used(),
            if matrix_service_tiles_enabled {
                PEAK_SRAM_BYTES
            } else {
                0
            }
        );
        assert_eq!(
            inspect.stats.borrow().written_bytes,
            if matrix_service_tiles_enabled {
                0
            } else {
                OUTPUT_BYTES
            }
        );
        Ok(())
    });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    println!(
        "direct runner matrix_service_tiles_enabled={matrix_service_tiles_enabled} completed_tick={}",
        end.get()
    );
    assert_eq!(final_inspect.sram.used(), 0);
    assert_eq!(
        final_inspect.stats.borrow().peak_sram_bytes,
        PEAK_SRAM_BYTES
    );
    assert_eq!(
        (memory.bytes_read(), memory.bytes_written()),
        (MATRIX_OPERAND_COUNT * LSU_ACCESS_BYTES, LSU_ACCESS_BYTES)
    );
    end.get()
}

fn run_concurrent_programs(replicas: bool) -> u64 {
    const PROGRAMS: usize = 2;
    let mut engine = Engine::default();
    let (runner, memory, done) = runner_with_matrix_service_tiles(&mut engine, true, replicas);
    let inspect = runner.clone();
    for id in 0..PROGRAMS {
        let runner = runner.clone();
        engine
            .spawner()
            .spawn(async move { runner.run_program(id, &skinny_matrix_program_task()).await });
    }
    engine.run().unwrap();
    assert_eq!(done.0.get(), PROGRAMS);
    assert_eq!(
        inspect.stats.borrow().machine_ops.by_operation["matrix_mac"],
        PROGRAMS * SKINNY_MACS
    );
    assert_eq!(
        (memory.bytes_read(), memory.bytes_written()),
        (
            PROGRAMS * MATRIX_OPERAND_COUNT * LSU_ACCESS_BYTES,
            PROGRAMS * LSU_ACCESS_BYTES
        )
    );
    assert_eq!(inspect.sram.used(), 0);
    engine.default_clock().tick_now().tick()
}

fn runner_with_matrix_service_tiles(
    engine: &mut Engine,
    matrix_service_tiles_enabled: bool,
    replicated: bool,
) -> (TaskRunner, Rc<Memory<MemoryAccess>>, Rc<Completed>) {
    matrix_runner_fixture(
        engine,
        &matrix_processing_element_config(
            MATRIX_MACS_PER_TICK,
            matrix_service_tiles_enabled,
            None,
            replicated,
        ),
    )
}

fn matrix_service_model(ops_per_tick: &str, tile: &str) -> ComputeModel {
    let config = from_str(&format!(
        r"
execution_units:
  matrix:
    capabilities:
      - class: matrix
        input_dtypes: [[bf16, bf16]]
        ops_per_tick: {ops_per_tick}
        accumulator: fp32
        matrix_service_tile: {tile}
"
    ))
    .unwrap();
    let classes = from_str("matrix: {operations: [matrix_mac],arity: 2}").unwrap();
    ComputeModel::new(&config, &classes).unwrap()
}

fn transposed_asymmetric_program() -> ProgramTaskConfig {
    let mut task = skinny_matrix_program_task();
    let mut value = to_value(&task.program).unwrap();
    value["assignments"][0]["axes"] = from_str("[n,m]").unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][0]["read"]["indices"] =
        from_str("[k,m]").unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][1]["read"]["indices"] =
        from_str("[n,k]").unwrap();
    task.program = from_value(value).unwrap();
    task.inputs = vec![
        full_tensor_view(
            &[ASYMMETRIC_K, ASYMMETRIC_M],
            DataType::Bf16,
            LHS_BASE_ADDRESS,
        ),
        full_tensor_view(
            &[ASYMMETRIC_N, ASYMMETRIC_K],
            DataType::Bf16,
            RHS_BASE_ADDRESS,
        ),
    ];
    task.outputs = vec![full_tensor_view(
        &[ASYMMETRIC_N, ASYMMETRIC_M],
        DataType::Fp32,
        OUTPUT_BASE_ADDRESS,
    )];
    task
}

fn skinny_matrix_program_task() -> ProgramTaskConfig {
    let mut task = matrix_program_task(SKINNY_M, SKINNY_N, SKINNY_K);
    task.id = "skinny".into();
    task
}

#[test]
fn matrix_service_padding_extends_sram_residency_with_unchanged_peak_and_traffic() {
    let unpadded_finish = run_service_residency(false);
    let padded_finish = run_service_residency(true);

    // Direct runner completion precedes the dispatcher simulation engine
    // shutdown tick.
    assert_eq!(unpadded_finish, PLAIN_FINISH_TICK);
    assert_eq!(
        padded_finish,
        unpadded_finish + PADDED_COMPUTE_TICKS - PLAIN_COMPUTE_TICKS
    );
}

#[test]
fn broadcast_matrix_service_uses_logical_shape_and_reads_only_present_values() {
    const REDUCTION_ELEMENTS: usize = SKINNY_K;
    const OUTPUT_COLUMNS: usize = SKINNY_N;
    let mut engine = Engine::default();
    let (runner, memory, done) = runner_with_matrix_service_tiles(&mut engine, true, false);
    let inspect = runner.clone();
    let mut p = skinny_matrix_program_task();
    p.inputs[1] = TensorView::new_full(
        Tensor::new(&[REDUCTION_ELEMENTS, 1], &DataType::Bf16, RHS_BASE_ADDRESS).unwrap(),
    );
    // Preserve the full output axis instead of inferring n=1 from B.
    p.program.extents.insert("n".into(), OUTPUT_COLUMNS);
    engine
        .spawner()
        .spawn(async move { runner.run_program(0, &p).await });
    engine.run().unwrap();
    assert_eq!(done.0.get(), 1);
    assert_eq!(inspect.sram.used(), 0);
    let input_bytes = MATRIX_OPERAND_COUNT * REDUCTION_ELEMENTS * BYTES_PER_BF16; // One row of A and one column of B.
    let output_bytes = OUTPUT_COLUMNS * BYTES_PER_FP32;
    assert_eq!(
        (
            inspect.stats.borrow().read_bytes,
            inspect.stats.borrow().written_bytes
        ),
        (input_bytes, output_bytes)
    );
    assert_eq!(
        inspect.stats.borrow().peak_sram_bytes,
        input_bytes + output_bytes
    );
    assert_eq!(
        (memory.bytes_read(), memory.bytes_written()),
        (MATRIX_OPERAND_COUNT * LSU_ACCESS_BYTES, LSU_ACCESS_BYTES)
    );
    assert_eq!(
        inspect.stats.borrow().machine_ops.by_operation["matrix_mac"],
        OUTPUT_COLUMNS * REDUCTION_ELEMENTS
    );
}

#[test]
fn concurrent_matrix_programs_finish_sooner_with_execution_unit_replicas() {
    let elapsed = [
        run_concurrent_programs(false),
        run_concurrent_programs(true),
    ];
    assert!(
        elapsed[0] > elapsed[1],
        "service on the same execution unit must contend: {elapsed:?}"
    );
    println!("native shared/independent matrix service ticks {elapsed:?}");
}

#[test]
fn streamed_compute_validation_uses_chunk_service_cost_when_full_reduction_would_overflow() {
    const SLOW_MACS_PER_TICK: &str = "0.000000001";
    let k = 1usize << 40;
    let mut task = skinny_matrix_program_task();
    task.inputs = vec![
        full_tensor_view(&[1, k], DataType::Bf16, 0),
        full_tensor_view(&[k, 1], DataType::Bf16, 1u64 << 43),
    ];
    task.outputs = vec![full_tensor_view(&[1, 1], DataType::Fp32, 1u64 << 44)];
    task.tiling = Some(from_str("sizes: {k: 1}").unwrap());
    let region = validate_program(
        &task.program,
        ProgramBindings {
            inputs: task.inputs.clone(),
            outputs: task.outputs.clone(),
        },
    )
    .unwrap();
    let service_tile =
        format!("{{m: {MATRIX_SERVICE_M}, n: {MATRIX_SERVICE_N}, k: {MATRIX_SERVICE_K}}}");
    let model = matrix_service_model(SLOW_MACS_PER_TICK, &service_tile);
    let schedule = plan_tiles(&region, task.tiling.as_ref()).unwrap();
    // Inspect one actual chunk lazily; this intentionally enormous schedule
    // must not be drained just to establish the chunk/full-duration
    // distinction.
    let tile = schedule.tiles().next().unwrap();
    let invocation = AssignmentInvocations::new(&region, 0, &tile)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    model.validate_work(&invocation.work).unwrap();
    let full = build_matrix_work(&region.program.assignments[0], &region, &BTreeMap::new())
        .unwrap()
        .unwrap();
    assert!(
        model.lower(&full).is_err(),
        "fictitious one-shot service overflows; no giant native run"
    );
}

#[test]
fn matrix_service_shape_uses_operand_indices_when_output_axes_are_reordered() {
    const MACS_PER_TICK: usize = 1;
    let task = transposed_asymmetric_program();
    let region = validate_program(
        &task.program,
        ProgramBindings {
            inputs: task.inputs,
            outputs: task.outputs,
        },
    )
    .unwrap();
    let service_tile = format!(
        "{{m: {ASYMMETRIC_SERVICE_M}, n: {ASYMMETRIC_SERVICE_N}, k: {ASYMMETRIC_SERVICE_K}}}"
    );
    let model = matrix_service_model(&MACS_PER_TICK.to_string(), &service_tile);
    let work = build_matrix_work(&region.program.assignments[0], &region, &BTreeMap::new())
        .unwrap()
        .unwrap();
    let TypedWork::MatrixMac { shape, .. } = &work.stages[0].operations[0] else {
        panic!("matrix work")
    };
    assert_eq!(
        shape.geometry(),
        MatrixWorkShape {
            batches: 1,
            m: ASYMMETRIC_M,
            n: ASYMMETRIC_N,
            k: ASYMMETRIC_K
        }
    );
    let service_macs = ASYMMETRIC_M.next_multiple_of(ASYMMETRIC_SERVICE_M)
        * ASYMMETRIC_N.next_multiple_of(ASYMMETRIC_SERVICE_N)
        * ASYMMETRIC_K.next_multiple_of(ASYMMETRIC_SERVICE_K);
    assert_eq!(
        model.lower(&work).unwrap().service_ticks().unwrap(),
        service_macs.div_ceil(MACS_PER_TICK) as u64
    );
    assert_eq!(
        work.stages[0].operations[0].count(),
        ASYMMETRIC_M * ASYMMETRIC_N * ASYMMETRIC_K
    );
}
