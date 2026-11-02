// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::cell::Cell;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_engine::engine::Engine;
use gwr_engine::events::repeated::Repeated;
use gwr_engine::traits::Event;
use gwr_engine::types::{SimError, SimResult};
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::task::{ProgramTaskConfig, Task};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use gwr_platform::Platform;
use yaml_serde::from_str;

// Platform resources and timing. Each input device and the output device has
// its own fabric port, alongside the PE.
const INPUT_DEVICE_COUNT: usize = 2;
const MEMORY_DEVICE_COUNT: usize = INPUT_DEVICE_COUNT + 1;
const FABRIC_COLUMNS: usize = MEMORY_DEVICE_COUNT + 1;
const INPUT_BASE_ADDRESS: u64 = 0;
const OUTPUT_BASE_ADDRESS: u64 = 64;
const OUTPUT_CAPACITY_BYTES: usize = 16;
const ACTIVE_REQUESTS: usize = 2;
const LSU_ACCESS_BYTES: usize = INT8_BYTES;
const COMPUTE_OPS_PER_TICK: usize = 8;
const MEMORY_DELAY_TICKS: usize = 1;
const FABRIC_PORT_BITS_PER_TICK: usize = 1024;

// Storage and cache geometry. One Int8 occupies a byte; two FP4 values share
// one.
const BITS_PER_BYTE: usize = 8;
const INT8_BYTES: usize = 1;
const FP4_BITS: usize = 4;
const FP4_ELEMENTS_PER_BYTE: usize = BITS_PER_BYTE / FP4_BITS;
const CACHE_LINE_BYTES: usize = 4;
const CACHE_SET_COUNT: usize = 16;
const CACHE_WAY_COUNT: usize = 1;
const CACHE_BYTES_PER_TICK: usize = 32;
const CACHE_DELAY_TICKS: usize = 1;

// Sparse rows are separated by an entire skipped cache line.
const SPARSE_ROWS: usize = 2;
const CACHE_LINES_PER_SPARSE_ROW: usize = 2;
const SPARSE_ROW_BYTES: usize = CACHE_LINES_PER_SPARSE_ROW * CACHE_LINE_BYTES;
const SPARSE_TENSOR_BYTES: usize = SPARSE_ROWS * SPARSE_ROW_BYTES;
const SPARSE_FOLLOWUP_ELEMENTS: usize = 1;
const SPARSE_SEQUENCE_TASKS: usize = 2;

// Leave one tensor-sized unmapped gap before the remote device.
const SPARSE_REMOTE_BASE: u64 = INPUT_BASE_ADDRESS + (SPARSE_TENSOR_BYTES * 2) as u64;

// Strided views select one element per row. Both offset cases below touch the
// same physical bytes, with the FP4 case starting in a byte's second nibble.
const STRIDED_ROWS: usize = 2;
const STRIDED_ROW_BYTES: usize = 4;
const FIRST_TOUCHED_BYTE_OFFSET: usize = 1;
const FIRST_TOUCHED_ADDRESS: u64 = INPUT_BASE_ADDRESS + FIRST_TOUCHED_BYTE_OFFSET as u64;
const SECOND_TOUCHED_ADDRESS: u64 = FIRST_TOUCHED_ADDRESS + STRIDED_ROW_BYTES as u64;
const OFFSET_CASES: [(DataType, usize, usize); 2] = [
    (
        DataType::Int8,
        STRIDED_ROW_BYTES / INT8_BYTES,
        FIRST_TOUCHED_BYTE_OFFSET / INT8_BYTES,
    ),
    (
        DataType::Fp4,
        STRIDED_ROW_BYTES * FP4_ELEMENTS_PER_BYTE,
        FIRST_TOUCHED_BYTE_OFFSET * FP4_ELEMENTS_PER_BYTE + 1,
    ),
];

// The contiguous fixture splits equal halves across adjacent input devices.
const ELEMENTS_PER_DEVICE: usize = 2;
const CONTIGUOUS_DEVICE_BYTES: usize = ELEMENTS_PER_DEVICE * INT8_BYTES;
const CONTIGUOUS_ELEMENTS: usize = INPUT_DEVICE_COUNT * ELEMENTS_PER_DEVICE;
const CONTIGUOUS_SECOND_BASE: u64 = INPUT_BASE_ADDRESS + CONTIGUOUS_DEVICE_BYTES as u64;

fn run_copy_and_check_traffic(
    source: &str,
    task: ProgramTaskConfig,
    expected: Option<(usize, usize)>,
) -> Platform {
    run_tasks_and_check_traffic(source, boxed_program_task(task), expected, 1)
}

fn run_tasks_and_check_traffic(
    source: &str,
    task: Task,
    expected: Option<(usize, usize)>,
    repeats: usize,
) -> Platform {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_string(&engine, &clock, source).unwrap();
    let dispatcher = task_dispatcher(task, None, repeats);
    let interface: Rc<dyn Dispatch> = dispatcher.clone();
    platform.attach_dispatcher(&interface);
    match expected {
        Some(traffic) => {
            engine.run().unwrap();
            assert_eq!(dispatcher.completed.get(), repeats);
            assert_eq!(platform.pe("pe0").unwrap().memory_traffic_bytes(), traffic);
        }
        None => {
            let error = engine.run().unwrap_err();
            assert!(
                error.to_string().contains("unmapped touched bytes"),
                "{error}"
            );
            assert_unmapped_task_was_not_started(&platform, &dispatcher);
        }
    }
    platform
}

fn assert_unmapped_task_was_not_started(platform: &Platform, dispatcher: &SingleTask) {
    assert!(!dispatcher.active.get());
    let pe = platform.pe("pe0").unwrap();
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert!(pe.machine_ops().by_operation.is_empty());
    for name in ["m0", "m1", "out"] {
        let memory = platform.memory(name).unwrap();
        assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
    }
}

fn run_sparse_read_then_unfetched_cache_line() -> Platform {
    let first_input = TensorView::new(
        Tensor::new(
            &[SPARSE_ROWS, SPARSE_ROW_BYTES / INT8_BYTES],
            &DataType::Int8,
            INPUT_BASE_ADDRESS,
        )
        .unwrap(),
        &[SPARSE_ROWS, 1],
        &[0, 0],
    )
    .unwrap();
    let first_output = TensorView::new_full(
        Tensor::new(&[SPARSE_ROWS], &DataType::Int8, OUTPUT_BASE_ADDRESS).unwrap(),
    );
    // Read from the cache line between the two previously selected row starts.
    let later_input = TensorView::new_full(
        Tensor::new(
            &[SPARSE_FOLLOWUP_ELEMENTS],
            &DataType::Int8,
            INPUT_BASE_ADDRESS + CACHE_LINE_BYTES as u64,
        )
        .unwrap(),
    );
    let later_output = TensorView::new_full(
        Tensor::new(
            &[SPARSE_FOLLOWUP_ELEMENTS],
            &DataType::Int8,
            OUTPUT_BASE_ADDRESS + CACHE_LINE_BYTES as u64,
        )
        .unwrap(),
    );
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_string(
        &engine,
        &clock,
        &platform_yaml_with_cache(
            &multi_device_platform_yaml(
                SPARSE_TENSOR_BYTES as u64,
                SPARSE_REMOTE_BASE,
                CACHE_LINE_BYTES as u64,
            ),
            CACHE_LINE_BYTES,
        ),
    )
    .unwrap();
    let dispatcher = task_dispatcher(
        boxed_program_task(copy_program_task(first_input, first_output, "[i, 0]")),
        Some(boxed_program_task(copy_program_task(
            later_input,
            later_output,
            "[i]",
        ))),
        SPARSE_SEQUENCE_TASKS,
    );
    let interface: Rc<dyn Dispatch> = dispatcher.clone();
    platform.attach_dispatcher(&interface);
    engine.run().unwrap();
    assert_eq!(dispatcher.completed.get(), SPARSE_SEQUENCE_TASKS);
    platform
}

fn copy_program_task(input: TensorView, output: TensorView, indices: &str) -> ProgramTaskConfig {
    ProgramTaskConfig {
        id: "copy".into(),
        program: from_str(&format!(
            r"
inputs: [A]
outputs: [Y]
assignments:
 - output: Y
   axes: [i]
   expr:
     read: {{ tensor: A, indices: {indices} }}
"
        ))
        .unwrap(),
        inputs: vec![input],
        outputs: vec![output],
        tiling: None,
    }
}

fn boxed_program_task(config: ProgramTaskConfig) -> Task {
    Task::ProgramTask {
        config: Box::new(config),
    }
}

fn task_dispatcher(task: Task, next_task: Option<Task>, repeats: usize) -> Rc<SingleTask> {
    Rc::new(SingleTask {
        task,
        next_task,
        active: Cell::new(false),
        completed: Cell::new(0),
        repeats,
        changed: Repeated::new(()),
    })
}

struct SingleTask {
    task: Task,
    next_task: Option<Task>,
    active: Cell<bool>,
    completed: Cell<usize>,
    repeats: usize,
    changed: Repeated<()>,
}

#[async_trait(?Send)]
impl Dispatch for SingleTask {
    fn task_by_id(&self, _: usize) -> Result<Task, SimError> {
        if self.completed.get() > 0
            && let Some(task) = &self.next_task
        {
            return Ok(task.clone());
        }
        Ok(self.task.clone())
    }

    fn set_task_active(&self, _: usize) -> SimResult {
        self.active.set(true);
        Ok(())
    }

    fn set_task_completed(&self, _: usize) -> SimResult {
        self.completed.set(self.completed.get() + 1);
        self.active.set(false);
        self.changed.notify();
        Ok(())
    }

    fn ready_task_indices(&self, _: &str) -> Result<(bool, Vec<usize>), SimError> {
        Ok((
            self.completed.get() == self.repeats,
            if self.active.get() { vec![] } else { vec![0] },
        ))
    }

    async fn wait_for_change(&self) {
        self.changed.listen().await;
    }

    fn total_tasks_for_pe(&self, _: &str) -> usize {
        self.repeats
    }
}

fn platform_yaml_with_cache(source: &str, line_bytes: usize) -> String {
    source.replace(
        " - {connect: [pe.pe0, 'fabric.f@(0,0)']}",
        " - {connect: [pe.pe0, cache.c.dev]}\n - {connect: [cache.c.mem, 'fabric.f@(0,0)']}",
    ) + &format!(
        r"
caches:
 - name: c
   config:
     line_size_bytes: {line_bytes}
     num_sets: {CACHE_SET_COUNT}
     num_ways: {CACHE_WAY_COUNT}
     bw_bytes_per_tick: {CACHE_BYTES_PER_TICK}
     delay_ticks: {CACHE_DELAY_TICKS}
"
    )
}

fn single_input_device_platform_yaml() -> String {
    multi_device_platform_yaml(
        CONTIGUOUS_DEVICE_BYTES as u64,
        CONTIGUOUS_SECOND_BASE,
        CONTIGUOUS_DEVICE_BYTES as u64,
    )
    .replacen("[m0, m1, out]", "[m0, out]", 1)
}

fn multi_device_platform_yaml(first_bytes: u64, second_base: u64, second_bytes: u64) -> String {
    format!(
        r"
memory_maps:
 - name: local
   devices: [m0, m1, out]
operation_classes:
 add: {{operations: [add], arity: 2}}
processing_elements:
 - name: pe0
   memory_map: local
   config:
    num_active_requests: {ACTIVE_REQUESTS}
    lsu_access_bytes: {LSU_ACCESS_BYTES}
    compute:
      execution_units:
        vector:
          capabilities:
            - class: add
              ops_per_tick: {COMPUTE_OPS_PER_TICK}
memories:
 - name: m0
   kind: hbm
   base_address: {INPUT_BASE_ADDRESS}
   config:
     capacity_bytes: {first_bytes}
     delay_ticks: {MEMORY_DELAY_TICKS}
 - name: m1
   kind: hbm
   base_address: {second_base}
   config:
     capacity_bytes: {second_bytes}
     delay_ticks: {MEMORY_DELAY_TICKS}
 - name: out
   kind: hbm
   base_address: {OUTPUT_BASE_ADDRESS}
   config:
     capacity_bytes: {OUTPUT_CAPACITY_BYTES}
     delay_ticks: {MEMORY_DELAY_TICKS}
fabrics:
 - name: f
   kind: functional
   columns: {FABRIC_COLUMNS}
   rows: 1
   config:
     fabric_ports_per_node: 1
     port_bits_per_tick: {FABRIC_PORT_BITS_PER_TICK}
     ticks_per_hop: 1
     ticks_overhead: 0
connections:
 - {{connect: [pe.pe0, 'fabric.f@(0,0)']}}
 - {{connect: [mem.m0, 'fabric.f@(1,0)']}}
 - {{connect: [mem.m1, 'fabric.f@(2,0)']}}
 - {{connect: [mem.out, 'fabric.f@(3,0)']}}
"
    )
}

fn nonzero_offset_views(dtype: DataType, width: usize, offset: usize) -> (TensorView, TensorView) {
    let input = TensorView::new(
        Tensor::new(&[STRIDED_ROWS, width], &dtype, INPUT_BASE_ADDRESS).unwrap(),
        &[STRIDED_ROWS, 1],
        &[0, offset],
    )
    .unwrap();
    let output =
        TensorView::new_full(Tensor::new(&[STRIDED_ROWS], &dtype, OUTPUT_BASE_ADDRESS).unwrap());
    (input, output)
}

#[test]
fn repeated_reads_across_devices_fetch_each_cache_line_once() {
    const REPEATS: usize = 2;
    const INPUT_ELEMENTS: usize = INPUT_DEVICE_COUNT * CACHE_LINE_BYTES / INT8_BYTES;
    const COPY_BYTES: usize = INPUT_ELEMENTS * INT8_BYTES;
    const TOTAL_COPY_BYTES: usize = REPEATS * COPY_BYTES;

    let input = TensorView::new_full(
        Tensor::new(&[INPUT_ELEMENTS], &DataType::Int8, INPUT_BASE_ADDRESS).unwrap(),
    );
    let output = TensorView::new_full(
        Tensor::new(&[INPUT_ELEMENTS], &DataType::Int8, OUTPUT_BASE_ADDRESS).unwrap(),
    );
    let second_base = INPUT_BASE_ADDRESS + CACHE_LINE_BYTES as u64;
    let platform = run_tasks_and_check_traffic(
        &platform_yaml_with_cache(
            &multi_device_platform_yaml(
                CACHE_LINE_BYTES as u64,
                second_base,
                CACHE_LINE_BYTES as u64,
            ),
            CACHE_LINE_BYTES,
        ),
        boxed_program_task(copy_program_task(input, output, "[i]")),
        Some((TOTAL_COPY_BYTES, TOTAL_COPY_BYTES)),
        REPEATS,
    );
    assert_eq!(
        platform.memory("m0").unwrap().bytes_read(),
        CACHE_LINE_BYTES
    );
    assert_eq!(
        platform.memory("m1").unwrap().bytes_read(),
        CACHE_LINE_BYTES
    );
    let cache = platform.cache("c").unwrap().traffic_snapshot();
    // Both active byte requests to a cold line miss before its fill completes;
    // they share one fill. All remaining requests, including the repeat, hit.
    let cold_misses = INPUT_DEVICE_COUNT * ACTIVE_REQUESTS;
    let read_requests = TOTAL_COPY_BYTES / LSU_ACCESS_BYTES;
    let hits = read_requests - cold_misses;
    assert_eq!(cache.request_misses, cold_misses);
    assert_eq!(cache.request_hits, hits);
    assert_eq!(cache.fill_requests, INPUT_DEVICE_COUNT);
}

#[test]
fn sparse_cache_reads_round_only_touched_lines() {
    let input = TensorView::new(
        Tensor::new(
            &[SPARSE_ROWS, SPARSE_ROW_BYTES / INT8_BYTES],
            &DataType::Int8,
            INPUT_BASE_ADDRESS,
        )
        .unwrap(),
        &[SPARSE_ROWS, 1],
        &[0, 0],
    )
    .unwrap();
    let output = TensorView::new_full(
        Tensor::new(&[SPARSE_ROWS], &DataType::Int8, OUTPUT_BASE_ADDRESS).unwrap(),
    );
    let payload_bytes = SPARSE_ROWS * INT8_BYTES;
    let platform = run_copy_and_check_traffic(
        &platform_yaml_with_cache(
            &multi_device_platform_yaml(
                SPARSE_TENSOR_BYTES as u64,
                SPARSE_REMOTE_BASE,
                CACHE_LINE_BYTES as u64,
            ),
            CACHE_LINE_BYTES,
        ),
        copy_program_task(input, output, "[i, 0]"),
        Some((payload_bytes, payload_bytes)),
    );
    // One selected byte per row causes one complete cache-line fetch per row.
    assert_eq!(
        platform.memory("m0").unwrap().bytes_read(),
        SPARSE_ROWS * CACHE_LINE_BYTES
    );
    let cache = platform.cache("c").unwrap().traffic_snapshot();
    assert_eq!(
        (cache.request_misses, cache.fill_requests),
        (SPARSE_ROWS, SPARSE_ROWS)
    );
}

#[test]
fn unmapped_cached_input_is_rejected_before_execution() {
    const REMOTE_ELEMENTS: usize = 2;
    let config = platform_yaml_with_cache(
        &multi_device_platform_yaml(
            SPARSE_TENSOR_BYTES as u64,
            SPARSE_REMOTE_BASE,
            CACHE_LINE_BYTES as u64,
        )
        .replacen("[m0, m1, out]", "[m0, out]", 1),
        CACHE_LINE_BYTES,
    );
    let remote = TensorView::new_full(
        Tensor::new(&[REMOTE_ELEMENTS], &DataType::Int8, SPARSE_REMOTE_BASE).unwrap(),
    );
    let output = TensorView::new_full(
        Tensor::new(&[REMOTE_ELEMENTS], &DataType::Int8, OUTPUT_BASE_ADDRESS).unwrap(),
    );
    run_copy_and_check_traffic(&config, copy_program_task(remote, output, "[i]"), None);
}

#[test]
fn cache_rejects_physical_boundaries_not_aligned_to_whole_lines() {
    const HALF_LINE_BYTES: usize = CACHE_LINE_BYTES / 2;
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let second_base = INPUT_BASE_ADDRESS + HALF_LINE_BYTES as u64;
    let error = match Platform::from_string(
        &engine,
        &clock,
        &platform_yaml_with_cache(
            &multi_device_platform_yaml(
                HALF_LINE_BYTES as u64,
                second_base,
                HALF_LINE_BYTES as u64,
            ),
            CACHE_LINE_BYTES,
        ),
    ) {
        Ok(_) => panic!("unaligned cache memory must fail platform validation"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains(&format!("{CACHE_LINE_BYTES}-byte fetch size")),
        "{error}"
    );
}

#[test]
fn reading_a_cache_line_skipped_by_a_sparse_read_causes_a_miss() {
    let platform = run_sparse_read_then_unfetched_cache_line();
    let selected_elements = SPARSE_ROWS + SPARSE_FOLLOWUP_ELEMENTS;
    let payload_bytes = selected_elements * INT8_BYTES;
    assert_eq!(
        platform.pe("pe0").unwrap().memory_traffic_bytes(),
        (payload_bytes, payload_bytes)
    );
    // Each selected element is in a distinct line, including the later read.
    let fetched_lines = selected_elements;
    assert_eq!(
        platform.memory("m0").unwrap().bytes_read(),
        fetched_lines * CACHE_LINE_BYTES
    );
    let cache = platform.cache("c").unwrap().traffic_snapshot();
    assert_eq!(
        (
            cache.request_misses,
            cache.request_hits,
            cache.fill_requests
        ),
        (fetched_lines, 0, fetched_lines)
    );
}

#[test]
fn strided_reads_allow_unmapped_gaps_between_selected_bytes() {
    let input = TensorView::new(
        Tensor::new(
            &[STRIDED_ROWS, STRIDED_ROW_BYTES / INT8_BYTES],
            &DataType::Int8,
            INPUT_BASE_ADDRESS,
        )
        .unwrap(),
        &[STRIDED_ROWS, 1],
        &[0, 0],
    )
    .unwrap();
    let output = TensorView::new_full(
        Tensor::new(&[STRIDED_ROWS], &DataType::Int8, OUTPUT_BASE_ADDRESS).unwrap(),
    );
    let selected_bytes = STRIDED_ROWS * INT8_BYTES;
    run_copy_and_check_traffic(
        &multi_device_platform_yaml(
            INT8_BYTES as u64,
            INPUT_BASE_ADDRESS + STRIDED_ROW_BYTES as u64,
            INT8_BYTES as u64,
        ),
        copy_program_task(input, output, "[i, 0]"),
        Some((selected_bytes, selected_bytes)),
    );
}

#[test]
fn strided_reads_reject_unmapped_selected_bytes_before_task_activation() {
    let input = TensorView::new(
        Tensor::new(
            &[STRIDED_ROWS, STRIDED_ROW_BYTES / INT8_BYTES],
            &DataType::Int8,
            INPUT_BASE_ADDRESS,
        )
        .unwrap(),
        &[STRIDED_ROWS, 1],
        &[0, 0],
    )
    .unwrap();
    let output = TensorView::new_full(
        Tensor::new(&[STRIDED_ROWS], &DataType::Int8, OUTPUT_BASE_ADDRESS).unwrap(),
    );
    // Move the second device one byte beyond the second selected element.
    let missing_second_base = INPUT_BASE_ADDRESS + (STRIDED_ROW_BYTES + INT8_BYTES) as u64;
    run_copy_and_check_traffic(
        &multi_device_platform_yaml(INT8_BYTES as u64, missing_second_base, INT8_BYTES as u64),
        copy_program_task(input, output, "[i, 0]"),
        None,
    );
}

#[test]
fn contiguous_reads_across_devices_charge_each_device_for_its_bytes() {
    let input = TensorView::new_full(
        Tensor::new(&[CONTIGUOUS_ELEMENTS], &DataType::Int8, INPUT_BASE_ADDRESS).unwrap(),
    );
    let output = TensorView::new_full(
        Tensor::new(&[CONTIGUOUS_ELEMENTS], &DataType::Int8, OUTPUT_BASE_ADDRESS).unwrap(),
    );
    let copy_bytes = CONTIGUOUS_ELEMENTS * INT8_BYTES;
    let platform = run_copy_and_check_traffic(
        &multi_device_platform_yaml(
            CONTIGUOUS_DEVICE_BYTES as u64,
            CONTIGUOUS_SECOND_BASE,
            CONTIGUOUS_DEVICE_BYTES as u64,
        ),
        copy_program_task(input, output, "[i]"),
        Some((copy_bytes, copy_bytes)),
    );
    assert_eq!(
        platform.memory("m0").unwrap().bytes_read(),
        CONTIGUOUS_DEVICE_BYTES
    );
    assert_eq!(
        platform.memory("m1").unwrap().bytes_read(),
        CONTIGUOUS_DEVICE_BYTES
    );
}

#[test]
fn unmapped_compute_inputs_are_rejected_before_task_activation() {
    let source = single_input_device_platform_yaml();
    let view = |addr| {
        TensorView::new_full(Tensor::new(&[ELEMENTS_PER_DEVICE], &DataType::Int8, addr).unwrap())
    };
    run_copy_and_check_traffic(
        &source,
        copy_program_task(
            view(CONTIGUOUS_SECOND_BASE),
            view(OUTPUT_BASE_ADDRESS),
            "[i]",
        ),
        None,
    );
}

#[test]
fn unmapped_compute_outputs_are_rejected_before_task_activation() {
    let source = single_input_device_platform_yaml();
    let view = |addr| {
        TensorView::new_full(Tensor::new(&[ELEMENTS_PER_DEVICE], &DataType::Int8, addr).unwrap())
    };
    run_copy_and_check_traffic(
        &source,
        copy_program_task(
            view(INPUT_BASE_ADDRESS),
            view(CONTIGUOUS_SECOND_BASE),
            "[i]",
        ),
        None,
    );
}

#[test]
fn nonzero_strided_and_packed_offsets_allow_untouched_unmapped_bytes() {
    for (dtype, width, offset) in OFFSET_CASES {
        let (input, output) = nonzero_offset_views(dtype, width, offset);
        let mapped = multi_device_platform_yaml(
            INT8_BYTES as u64,
            SECOND_TOUCHED_ADDRESS,
            INT8_BYTES as u64,
        )
        .replacen(
            &format!("base_address: {INPUT_BASE_ADDRESS}\n"),
            &format!("base_address: {FIRST_TOUCHED_ADDRESS}\n"),
            1,
        );
        // One physical input byte per row, even when only a nibble is selected.
        let input_bytes = STRIDED_ROWS;
        let output_bytes = (STRIDED_ROWS * dtype.num_bits()).div_ceil(BITS_PER_BYTE);
        run_copy_and_check_traffic(
            &mapped,
            copy_program_task(input, output, "[i, 0]"),
            Some((input_bytes, output_bytes)),
        );
    }
}

#[test]
fn nonzero_strided_and_packed_offsets_reject_missing_touched_bytes() {
    for (dtype, width, offset) in OFFSET_CASES {
        let (input, output) = nonzero_offset_views(dtype, width, offset);
        let missing_second_base = SECOND_TOUCHED_ADDRESS + INT8_BYTES as u64;
        let missing =
            multi_device_platform_yaml(INT8_BYTES as u64, missing_second_base, INT8_BYTES as u64)
                .replacen(
                    &format!("base_address: {INPUT_BASE_ADDRESS}\n"),
                    &format!("base_address: {FIRST_TOUCHED_ADDRESS}\n"),
                    1,
                );
        run_copy_and_check_traffic(&missing, copy_program_task(input, output, "[i, 0]"), None);
    }
}

#[test]
fn packed_views_count_unique_physical_bytes() {
    let view = TensorView::new(
        Tensor::new(
            &[STRIDED_ROWS, STRIDED_ROW_BYTES * FP4_ELEMENTS_PER_BYTE],
            &DataType::Fp4,
            INPUT_BASE_ADDRESS,
        )
        .unwrap(),
        &[STRIDED_ROWS, 1],
        &[0, 0],
    )
    .unwrap();
    let output = TensorView::new_full(
        Tensor::new(&[STRIDED_ROWS], &DataType::Fp4, OUTPUT_BASE_ADDRESS).unwrap(),
    );
    let input_bytes = STRIDED_ROWS;
    let output_bytes = (STRIDED_ROWS * FP4_BITS).div_ceil(BITS_PER_BYTE);
    run_copy_and_check_traffic(
        &multi_device_platform_yaml(
            INT8_BYTES as u64,
            INPUT_BASE_ADDRESS + STRIDED_ROW_BYTES as u64,
            INT8_BYTES as u64,
        ),
        copy_program_task(view, output, "[i, 0]"),
        Some((input_bytes, output_bytes)),
    );
}

#[test]
fn compute_can_read_the_final_physical_byte() {
    let input = TensorView::new_full(Tensor::new(&[1], &DataType::Int8, u64::MAX).unwrap());
    let output =
        TensorView::new_full(Tensor::new(&[1], &DataType::Int8, OUTPUT_BASE_ADDRESS).unwrap());
    let platform = run_copy_and_check_traffic(
        &multi_device_platform_yaml(INT8_BYTES as u64, u64::MAX, INT8_BYTES as u64),
        copy_program_task(input, output, "[i]"),
        Some((INT8_BYTES, INT8_BYTES)),
    );
    assert_eq!(platform.memory("m1").unwrap().bytes_read(), INT8_BYTES);
}
