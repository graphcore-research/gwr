// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Real PE/LSU controls for optional caller matrix service geometry.
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use async_trait::async_trait;
use futures::future::pending;
use gwr_engine::engine::Engine;
use gwr_engine::types::{SimError, SimResult};
use yaml_serde::{from_str, from_value, to_value};

use crate::processing_element::dispatch::Dispatch;
use crate::processing_element::matrix_test_fixture::*;
use crate::processing_element::program::syntax::TileSpec;
use crate::processing_element::program::tile::plan_tiles;
use crate::processing_element::program::validate::{RegionBindings, validate_program};
use crate::processing_element::task::{ProgramTaskConfig, Task};
use crate::processing_element::tensors::dtype::DataType;
use crate::processing_element::tensors::{Tensor, TensorView};

fn run_matrix_and_check_traffic(
    m: usize,
    n: usize,
    k: usize,
    matrix_service_tiles_enabled: bool,
) -> (u64, usize, usize) {
    let observed =
        execute_matrix_program(matrix_program_task(m, n, k), matrix_service_tiles_enabled);
    observed.result.unwrap();
    assert_eq!(observed.task_state, TaskState::Completed);
    assert_eq!(observed.ops, m * n * k);
    let traffic = (BYTES_PER_BF16 * (m * k + k * n), BYTES_PER_FP32 * m * n);
    assert_eq!(observed.traffic, traffic);
    assert_eq!(
        observed.backing,
        (MATRIX_OPERAND_COUNT * LSU_ACCESS_BYTES, LSU_ACCESS_BYTES)
    );
    println!(
        "matrix m={m} n={n} k={k} service_tiles={matrix_service_tiles_enabled} ticks={} MAC={} read={} write={}",
        observed.ticks,
        m * n * k,
        traffic.0,
        traffic.1
    );
    (observed.ticks, traffic.0, traffic.1)
}

fn execute_matrix_program(task: ProgramTaskConfig, matrix_service_tiles_enabled: bool) -> Observed {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let mut config = matrix_processing_element_config(
        MATRIX_MACS_PER_TICK,
        matrix_service_tiles_enabled,
        None,
        false,
    );
    // Retain the integration fixture's original execution unit identity.
    let matrix = config.compute.execution_units.remove("matrix0").unwrap();
    config
        .compute
        .execution_units
        .insert("matrix".into(), matrix);
    let (pe, memory) = processing_element_and_memory_fixture(&mut engine, &config, "hbm");
    let dispatch = Rc::new(OneTask {
        fetched: Cell::new(false),
        task: Task::ProgramTask {
            config: Box::new(task),
        },
        state: Cell::new(TaskState::Pending),
    });
    let dyn_dispatch: Rc<dyn Dispatch> = dispatch.clone();
    pe.set_dispatcher(&dyn_dispatch);
    let result = engine.run();
    Observed {
        result,
        ticks: clock.tick_now().tick(),
        task_state: dispatch.state.get(),
        ops: pe.machine_ops().checked_operation_total().unwrap(),
        traffic: pe.memory_traffic_bytes(),
        backing: (memory.bytes_read(), memory.bytes_written()),
    }
}

struct OneTask {
    task: Task,
    fetched: Cell<bool>,
    state: Cell<TaskState>,
}

#[async_trait(?Send)]
impl Dispatch for OneTask {
    fn task_by_id(&self, _: usize) -> Result<Task, SimError> {
        assert!(
            !self.fetched.replace(true),
            "dispatch task must be fetched exactly once"
        );
        Ok(self.task.clone())
    }

    fn set_task_active(&self, _: usize) -> SimResult {
        self.state.set(TaskState::Active);
        Ok(())
    }

    fn set_task_completed(&self, _: usize) -> SimResult {
        self.state.set(TaskState::Completed);
        Ok(())
    }

    fn ready_task_indices(&self, _: &str) -> Result<(bool, Vec<usize>), SimError> {
        let pending = self.state.get() == TaskState::Pending;
        Ok((!pending, if pending { vec![0] } else { vec![] }))
    }

    async fn wait_for_change(&self) {
        pending::<()>().await;
    }

    fn total_tasks_for_pe(&self, _: &str) -> usize {
        1
    }
}

struct Observed {
    result: Result<(), SimError>,
    ticks: u64,
    task_state: TaskState,
    ops: usize,
    traffic: (usize, usize),
    backing: (usize, usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskState {
    Pending,
    Active,
    Completed,
}

fn batched_matrix_program_task(
    batches: usize,
    rows: usize,
    columns: usize,
    reduction_elements: usize,
) -> ProgramTaskConfig {
    let mut task = matrix_program_task(rows, columns, reduction_elements);
    let mut value = to_value(&task.program).unwrap();
    value["assignments"][0]["axes"] = from_str("[b,m,n]").unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][0]["read"]["indices"] =
        from_str("[b,m,k]").unwrap();
    value["assignments"][0]["expr"]["matrix_mac"]["args"][1]["read"]["indices"] =
        from_str("[b,k,n]").unwrap();
    task.program = from_value(value).unwrap();
    task.inputs[0] = TensorView::new_full(
        Tensor::new(
            &[batches, rows, reduction_elements],
            &DataType::Bf16,
            LHS_BASE_ADDRESS,
        )
        .unwrap(),
    );
    task.inputs[1] = TensorView::new_full(
        Tensor::new(
            &[batches, reduction_elements, columns],
            &DataType::Bf16,
            RHS_BASE_ADDRESS,
        )
        .unwrap(),
    );
    task.outputs[0] = TensorView::new_full(
        Tensor::new(
            &[batches, rows, columns],
            &DataType::Fp32,
            OUTPUT_BASE_ADDRESS,
        )
        .unwrap(),
    );
    task
}

#[test]
fn service_tiles_increase_runner_time_for_skinny_matrices_with_equal_mac_counts() {
    const M: usize = MATRIX_SERVICE_M;
    const N: usize = MATRIX_SERVICE_N;
    const K: usize = MATRIX_SERVICE_K;
    const SKINNY_N: usize = M * N;

    // Compare one full service block with a one-row matrix containing the
    // same number of output elements. Both perform M * N * K useful MACs.
    let plain_square = run_matrix_and_check_traffic(M, N, K, false);
    let plain_skinny = run_matrix_and_check_traffic(1, SKINNY_N, K, false);

    let logical_macs = M * N * K;
    let logical_compute_ticks = logical_macs.div_ceil(MATRIX_MACS_PER_TICK) as u64;

    // Memory replies before consuming a packet's bandwidth service time.
    // The second operand follows the first by one packet interval; simulation
    // engine shutdown also drains the final output packet's service interval.
    let input_response_gap_ticks = (MATRIX_OPERAND_COUNT - 1) as u64 * MEMORY_PACKET_SERVICE_TICKS;
    let output_drain_ticks = MEMORY_PACKET_SERVICE_TICKS;
    assert_eq!(
        plain_square.0,
        input_response_gap_ticks + logical_compute_ticks + output_drain_ticks,
        "logical MACs determine service when service tiles are omitted"
    );
    assert_eq!(
        plain_square.0, plain_skinny.0,
        "one coalesced packet per operand preserves external timing control"
    );

    // With service tiles, the square still fills one block. The skinny
    // matrix is charged M rows for each group of N columns, so its padded
    // MAC count determines the additional compute time.
    let square = run_matrix_and_check_traffic(M, N, K, true);
    let skinny = run_matrix_and_check_traffic(1, SKINNY_N, K, true);
    assert_eq!(square.0, plain_square.0);

    let skinny_service_macs = 1usize.next_multiple_of(MATRIX_SERVICE_M)
        * SKINNY_N.next_multiple_of(MATRIX_SERVICE_N)
        * K.next_multiple_of(MATRIX_SERVICE_K);
    let skinny_compute_ticks = skinny_service_macs.div_ceil(MATRIX_MACS_PER_TICK) as u64;
    assert_eq!(
        skinny.0,
        square.0 + skinny_compute_ticks - logical_compute_ticks
    );

    // Padding affects compute time; payload sizes follow the logical shapes.
    assert_eq!(
        (square.1, square.2),
        ((M * K + K * N) * BYTES_PER_BF16, M * N * BYTES_PER_FP32,)
    );
    assert_eq!(
        (skinny.1, skinny.2),
        (
            (K + K * SKINNY_N) * BYTES_PER_BF16,
            SKINNY_N * BYTES_PER_FP32,
        )
    );
}

#[test]
fn broadcast_and_shared_free_axes_execute_with_service_tiles() {
    const M: usize = MATRIX_SERVICE_M;
    const N: usize = MATRIX_SERVICE_N;
    const K: usize = MATRIX_SERVICE_K;

    // Broadcast: one physical A row supplies every logical m position.
    // Shared free axis: Y[m,n] = sum_k A[m,k] * B[m,k], so each m selects
    // a row dot product and n repeats that result across output columns.
    for kind in ["broadcast", "shared"] {
        let mut config = matrix_program_task(M, N, K);
        config.program.extents.extend([
            (String::from("m"), M),
            (String::from("n"), N),
            (String::from("k"), K),
        ]);

        if kind == "broadcast" {
            config.inputs[0] = TensorView::new_full(
                Tensor::new(&[1, K], &DataType::Bf16, LHS_BASE_ADDRESS).unwrap(),
            );
        } else {
            let mut value = to_value(&config.program).unwrap();
            value["assignments"][0]["expr"]["matrix_mac"]["args"][1]["read"]["indices"] =
                from_str("[m,k]").unwrap();
            config.program = from_value(value).unwrap();
        }

        let plain = execute_matrix_program(config.clone(), false);
        plain.result.unwrap();
        assert_eq!(plain.task_state, TaskState::Completed);

        // Both configurations must execute the same logical work and
        // traffic.
        let with_service_tiles = execute_matrix_program(config, true);
        with_service_tiles.result.unwrap();
        assert_eq!(with_service_tiles.task_state, TaskState::Completed);
        assert_eq!(with_service_tiles.ops, plain.ops);
        assert_eq!(with_service_tiles.traffic, plain.traffic);
        assert_eq!(with_service_tiles.backing, plain.backing);

        let outputs = M * N;
        let logical_macs = outputs * K;

        // The shared-axis case charges one dot-product block per (m,n)
        // output position, including the repeated n positions. The broadcast
        // case retains a full logical M-by-N matrix and fills one block,
        // even though its physical A tensor has only one row.
        let service_macs = if kind == "shared" {
            outputs * MATRIX_SERVICE_M * MATRIX_SERVICE_N * MATRIX_SERVICE_K
        } else {
            logical_macs
        };
        let extra_ticks = (service_macs.div_ceil(MATRIX_MACS_PER_TICK)
            - logical_macs.div_ceil(MATRIX_MACS_PER_TICK)) as u64;
        assert_eq!(with_service_tiles.ticks, plain.ticks + extra_ticks);
    }
}

#[test]
fn matrix_service_tiles_round_each_reduction_chunk_independently() {
    const M: usize = MATRIX_SERVICE_M;
    const N: usize = MATRIX_SERVICE_N;
    const CONTRACTION_SERVICE_BLOCKS: usize = 2;
    const K: usize = CONTRACTION_SERVICE_BLOCKS * MATRIX_SERVICE_K;
    const FIRST_CHUNK_K: usize = MATRIX_SERVICE_K + 1;
    const TAIL_CHUNK_K: usize = K - FIRST_CHUNK_K;
    const CHUNKS: usize = K.div_ceil(FIRST_CHUNK_K);

    // Split two contraction blocks one element past the first boundary.
    // The first chunk rounds to two service blocks and the tail to one.
    let mut chunked = matrix_program_task(M, N, K);
    chunked.tiling = Some(TileSpec {
        sizes: BTreeMap::from([("k".into(), FIRST_CHUNK_K)]),
    });

    // Check the logical output tile and contraction intervals first.
    let region = validate_program(
        &chunked.program,
        RegionBindings {
            inputs: chunked.inputs.clone(),
            outputs: chunked.outputs.clone(),
        },
    )
    .unwrap();
    let schedule = plan_tiles(&region, chunked.tiling.as_ref()).unwrap();
    let tile = schedule.tiles().next().unwrap();
    assert_eq!(tile.output_intervals["m"], 0..M);
    assert_eq!(tile.output_intervals["n"], 0..N);

    let chunks = tile
        .reduction_chunks_for(&["k".into()])
        .map(|chunk| chunk["k"].clone())
        .collect::<Vec<_>>();
    assert_eq!(chunks, vec![0..FIRST_CHUNK_K, FIRST_CHUNK_K..K]);
    println!("actual reduction chunk intervals {chunks:?}");

    // Execute the same chunks with and without padded service timing.
    let baseline = execute_matrix_program(chunked.clone(), false);
    baseline.result.unwrap();
    let selected = execute_matrix_program(chunked, true);
    selected.result.unwrap();

    let logical_macs = M * N * K;
    let service_macs = M
        * N
        * (FIRST_CHUNK_K.next_multiple_of(MATRIX_SERVICE_K)
            + TAIL_CHUNK_K.next_multiple_of(MATRIX_SERVICE_K));
    let extra_compute_ticks = (service_macs.div_ceil(MATRIX_MACS_PER_TICK)
        - logical_macs.div_ceil(MATRIX_MACS_PER_TICK)) as u64;
    assert_eq!(selected.ticks, baseline.ticks + extra_compute_ticks);

    // Useful MAC counts and payload bytes still cover the original tensors.
    assert_eq!(selected.ops, M * N * K);
    assert_eq!(
        selected.traffic,
        ((M * K + K * N) * BYTES_PER_BF16, M * N * BYTES_PER_FP32,)
    );

    // Each chunk has M separate strided A row ranges and one contiguous B
    // range. Every range issues a full LSU packet, including tail ranges.
    let read_requests = CHUNKS * (M + 1);
    assert_eq!(
        selected.backing,
        (read_requests * LSU_ACCESS_BYTES, LSU_ACCESS_BYTES)
    );

    // An unsplit contraction fills exactly two blocks and needs no padding.
    assert_eq!(
        run_matrix_and_check_traffic(M, N, K, true).0,
        run_matrix_and_check_traffic(M, N, K, false).0
    );

    println!(
        "chunked K{FIRST_CHUNK_K}+{TAIL_CHUNK_K} logical{logical_macs} service{service_macs} baseline_ticks={} selected_ticks={}",
        baseline.ticks, selected.ticks
    );
}

#[test]
fn transposed_direct_operands_and_tails_use_symbolic_roles() {
    const M: usize = 2;
    const N: usize = 3;
    const K: usize = 5;

    // Ordinary orientation is the timing control. Both tensors fit one LSU
    // packet, so changing orientation does not change the memory service here.
    let ordinary_ticks = run_matrix_and_check_traffic(M, N, K, false).0;
    let logical_macs = M * N * K;
    let service_macs = M.next_multiple_of(MATRIX_SERVICE_M)
        * N.next_multiple_of(MATRIX_SERVICE_N)
        * K.next_multiple_of(MATRIX_SERVICE_K);
    let extra_compute_ticks = (service_macs.div_ceil(MATRIX_MACS_PER_TICK)
        - logical_macs.div_ceil(MATRIX_MACS_PER_TICK)) as u64;

    // Exercise the first operand only, the second only, and both together.
    // Indices describe already-transposed storage; no transpose task is added.
    for (transpose_lhs, transpose_rhs) in [(true, false), (false, true), (true, true)] {
        let mut task = matrix_program_task(M, N, K);
        let mut value = to_value(&task.program).unwrap();
        if transpose_lhs {
            value["assignments"][0]["expr"]["matrix_mac"]["args"][0]["read"]["indices"] =
                from_str("[k,m]").unwrap();
            task.inputs[0] = TensorView::new_full(
                Tensor::new(&[K, M], &DataType::Bf16, LHS_BASE_ADDRESS).unwrap(),
            );
        }
        if transpose_rhs {
            value["assignments"][0]["expr"]["matrix_mac"]["args"][1]["read"]["indices"] =
                from_str("[n,k]").unwrap();
            task.inputs[1] = TensorView::new_full(
                Tensor::new(&[N, K], &DataType::Bf16, RHS_BASE_ADDRESS).unwrap(),
            );
        }
        task.program = from_value(value).unwrap();

        let plain = execute_matrix_program(task.clone(), false);
        plain.result.unwrap();
        let shaped = execute_matrix_program(task, true);
        shaped.result.unwrap();
        assert_eq!(plain.task_state, TaskState::Completed);
        assert_eq!(shaped.task_state, TaskState::Completed);

        // The execution unit accepts every orientation without extra compute
        // work. Only the configured service-tile padding adds time in this
        // fixture.
        assert_eq!(plain.ticks, ordinary_ticks);
        assert_eq!(shaped.ticks, ordinary_ticks + extra_compute_ticks);
        assert_eq!(plain.ops, logical_macs);
        assert_eq!(shaped.ops, logical_macs);

        // Full-operand loads preserve payload sizes and packet counts here.
        let payload_bytes = ((M * K + K * N) * BYTES_PER_BF16, M * N * BYTES_PER_FP32);
        let packet_bytes = (MATRIX_OPERAND_COUNT * LSU_ACCESS_BYTES, LSU_ACCESS_BYTES);
        assert_eq!(plain.traffic, payload_bytes);
        assert_eq!(shaped.traffic, payload_bytes);
        assert_eq!(plain.backing, packet_bytes);
        assert_eq!(shaped.backing, packet_bytes);
    }
}

#[test]
fn batched_matrix_runs_with_or_without_service_tiles() {
    const BATCHES: usize = 2;
    const M: usize = MATRIX_SERVICE_M;
    const N: usize = MATRIX_SERVICE_N;
    const K: usize = MATRIX_SERVICE_K;

    // Each independent matrix fills one service block without padded work.
    let task = batched_matrix_program_task(BATCHES, M, N, K);

    let plain = execute_matrix_program(task.clone(), false);
    plain.result.unwrap();

    assert_eq!(plain.ops, BATCHES * M * N * K);
    assert_eq!(plain.task_state, TaskState::Completed);
    assert_eq!(
        plain.traffic,
        (
            BATCHES * (M * K + K * N) * BYTES_PER_BF16,
            BATCHES * M * N * BYTES_PER_FP32,
        )
    );

    // Enabling service tiles must preserve batch timing and both traffic
    // totals.
    let selected = execute_matrix_program(task, true);
    selected.result.unwrap();
    assert_eq!(selected.task_state, TaskState::Completed);
    assert_eq!(selected.ops, plain.ops);
    assert_eq!(selected.traffic, plain.traffic);
    assert_eq!(selected.backing, plain.backing);
    assert_eq!(selected.ticks, plain.ticks);
}

#[test]
fn batched_matrix_execution_handles_partial_batch_and_contraction_tiles() {
    const BATCHES: usize = 3;
    const ROWS: usize = 2;
    const COLUMNS: usize = 3;
    const REDUCTION_ELEMENTS: usize = 5;
    const BATCH_TILE: usize = BATCHES - 1;
    const REDUCTION_CHUNKS: usize = 2;
    const REDUCTION_CHUNK: usize = REDUCTION_ELEMENTS.div_ceil(REDUCTION_CHUNKS);

    // Leave one matrix in the final batch tile and a shorter final k chunk.
    // Both tails must execute and contribute their actual logical work.
    let mut task = batched_matrix_program_task(BATCHES, ROWS, COLUMNS, REDUCTION_ELEMENTS);
    task.tiling = Some(TileSpec {
        sizes: BTreeMap::from([("b".into(), BATCH_TILE), ("k".into(), REDUCTION_CHUNK)]),
    });

    let observed = execute_matrix_program(task, true);
    observed.result.unwrap();
    assert_eq!(observed.task_state, TaskState::Completed);

    // Every input element is read once and every batch output is written
    // once.
    assert_eq!(observed.ops, BATCHES * ROWS * COLUMNS * REDUCTION_ELEMENTS);
    let input_bytes = BATCHES * (ROWS + COLUMNS) * REDUCTION_ELEMENTS * BYTES_PER_BF16;
    let output_bytes = BATCHES * ROWS * COLUMNS * BYTES_PER_FP32;
    assert_eq!(observed.traffic, (input_bytes, output_bytes));
}

#[test]
fn raw_dispatch_preparation_rejections_have_no_side_effects() {
    for (name, axes, output_shape, expression, rate, expected, sram_bytes, flop_cost, tile) in [
        (
            "actual_duration",
            "[i]",
            vec![2],
            "exp: [{read: {tensor: A, indices: [i]}}]",
            "1e-19",
            "duration",
            16,
            1,
            None,
        ),
        (
            "intrinsic_serial_duration",
            "[i]",
            vec![2],
            "exp: [{exp: [{read: {tensor: A, indices: [i]}}]}]",
            "2e-19",
            "duration",
            64,
            1,
            None,
        ),
        (
            "program_accounting",
            "[i]",
            vec![2],
            "exp: [{read: {tensor: A, indices: [i]}}]",
            "1",
            "FLOP-equivalent count overflows",
            32,
            usize::MAX / 2 + 1,
            Some(1),
        ),
        (
            "scalar_empty",
            "[]",
            vec![],
            "reduce: {op: max, over: [k], value: {read: {tensor: A, indices: [{affine: {terms: {k: 3}, offset: -1}}], bounds: exclude}}}",
            "1",
            "empty",
            64,
            1,
            None,
        ),
        (
            "interior_empty",
            "[i]",
            vec![3],
            "reduce: {op: max, over: [k], value: {read: {tensor: A, indices: [{affine: {terms: {i: 1, k: 3}, offset: -2}}], bounds: exclude}}}",
            "1",
            "empty",
            64,
            1,
            None,
        ),
    ] {
        println!("case: {name}");
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        let mut config = matrix_processing_element_config(1, false, None, false);
        config.compute = from_str(&format!("execution_units: {{scalar: {{capabilities: [{{class: exp, input_dtypes: [[fp32]], ops_per_tick: {rate}}}, {{class: compare, input_dtypes: [[fp32,fp32]], ops_per_tick: 1}}]}}}}")).unwrap();
        config.operation_classes = from_str(
            "exp: {operations: [exp], arity: 1}\ncompare: {operations: [compare], arity: 2}",
        )
        .unwrap();
        config
            .operation_classes
            .get_mut("exp")
            .unwrap()
            .flops_per_op = flop_cost;
        config.sram_bytes = sram_bytes;
        let (pe, memory) = processing_element_and_memory_fixture(&mut engine, &config, "memory");
        let task = ProgramTaskConfig {
            id: "preflight".into(), tiling: tile.map(|size| TileSpec { sizes: BTreeMap::from([("i".into(), size)]) }),
            program: from_str(&format!("inputs: [A]\noutputs: [Y]\nextents: {{k: 2, i: {}}}\nassignments:\n - output: Y\n   axes: {axes}\n   expr: {{{expression}}}\n", output_shape.first().copied().unwrap_or(1))).unwrap(),
            inputs: vec![full_tensor_view(&[2], DataType::Fp32, LHS_BASE_ADDRESS)],
            outputs: vec![full_tensor_view(&output_shape, DataType::Fp32, OUTPUT_BASE_ADDRESS)],
        };
        let dispatch = Rc::new(OneTask {
            fetched: Cell::new(false),
            task: Task::ProgramTask {
                config: Box::new(task),
            },
            state: Cell::new(TaskState::Pending),
        });
        let dyn_dispatch: Rc<dyn Dispatch> = dispatch.clone();
        pe.set_dispatcher(&dyn_dispatch);
        let error = engine.run().unwrap_err();
        assert!(error.0.contains(expected), "{error}");
        assert_eq!(dispatch.state.get(), TaskState::Pending);
        assert_eq!(clock.tick_now().tick(), 0);
        assert_eq!(pe.machine_ops().checked_operation_total().unwrap(), 0);
        assert_eq!(pe.machine_ops().flop_equivalents(), 0);
        assert_eq!(pe.peak_sram_bytes(), 0);
        assert_eq!(pe.memory_traffic_bytes(), (0, 0));
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
    }
}

#[test]
fn prepared_task_rejects_another_pe_with_the_same_name_before_activation() {
    let config = matrix_processing_element_config(1, false, None, false);
    let mut original_engine = Engine::default();
    let (original, _) =
        processing_element_and_memory_fixture(&mut original_engine, &config, "memory");
    let prepared = original
        .prepare_program_task(matrix_program_task(1, 1, 1))
        .unwrap();
    let mut engine = Engine::default();
    let (other, memory) = processing_element_and_memory_fixture(&mut engine, &config, "memory");
    let dispatch = Rc::new(OneTask {
        fetched: Cell::new(false),
        task: Task::PreparedProgram { task: prepared },
        state: Cell::new(TaskState::Pending),
    });
    let dyn_dispatch: Rc<dyn Dispatch> = dispatch.clone();
    other.set_dispatcher(&dyn_dispatch);
    let error = engine.run().unwrap_err();
    assert!(error.0.contains("another processing element"), "{error}");
    assert_eq!(dispatch.state.get(), TaskState::Pending);
    assert_eq!(engine.default_clock().tick_now().tick(), 0);
    assert_eq!(other.peak_sram_bytes(), 0);
    assert_eq!(other.machine_ops().checked_operation_total().unwrap(), 0);
    assert_eq!(other.memory_traffic_bytes(), (0, 0));
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
}
