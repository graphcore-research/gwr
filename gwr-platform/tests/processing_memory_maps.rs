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
use gwr_platform::types::PlatformConfig;
use gwr_platform::yaml::platform_to_yaml_str;

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
        if self.completed.get() > 0 {
            if let Some(task) = &self.next_task {
                return Ok(task.clone());
            }
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

fn source(first_bytes: u64, second_base: u64, second_bytes: u64) -> String {
    format!(
        r#"
memory_maps:
 - name: local
   devices: [m0, m1, out]
operation_classes:
 add: {{operations: [add], arity: 2}}
processing_elements:
 - name: pe0
   memory_map: local
   config:
    num_active_requests: 2
    lsu_access_bytes: 1
    compute: {{engines: {{vector: {{capabilities: [{{class: add, ops_per_tick: 8}}]}}}}}}
memories:
 - {{name: m0, kind: hbm, base_address: 0, config: {{capacity_bytes: {first_bytes}, delay_ticks: 1}}}}
 - {{name: m1, kind: hbm, base_address: {second_base}, config: {{capacity_bytes: {second_bytes}, delay_ticks: 1}}}}
 - {{name: out, kind: hbm, base_address: 64, config: {{capacity_bytes: 16, delay_ticks: 1}}}}
fabrics:
 - name: f
   kind: functional
   columns: 4
   rows: 1
   config: {{fabric_ports_per_node: 1, port_bits_per_tick: 1024, ticks_per_hop: 1, ticks_overhead: 0}}
connections:
 - {{connect: [pe.pe0, 'fabric.f@(0,0)']}}
 - {{connect: [mem.m0, 'fabric.f@(1,0)']}}
 - {{connect: [mem.m1, 'fabric.f@(2,0)']}}
 - {{connect: [mem.out, 'fabric.f@(3,0)']}}
"#
    )
}

fn copy(input: TensorView, output: TensorView, indices: &str) -> ProgramTaskConfig {
    ProgramTaskConfig {
        pipeline: None,
        id: "copy".into(),
        program: yaml_serde::from_str(&format!("inputs: [A]\noutputs: [Y]\nassignments:\n - output: Y\n   axes: [i]\n   expr: {{read: {{tensor: A, indices: {indices}}}}}\n")).unwrap(),
        inputs: vec![input], outputs: vec![output], tiling: None,
    }
}

fn execute(source: &str, task: ProgramTaskConfig, expected: Option<(usize, usize)>) -> Platform {
    execute_task(
        source,
        Task::ProgramTask {
            config: Box::new(task),
        },
        expected,
    )
}

fn execute_task(source: &str, task: Task, expected: Option<(usize, usize)>) -> Platform {
    execute_repeated(source, task, expected, 1)
}

fn execute_repeated(
    source: &str,
    task: Task,
    expected: Option<(usize, usize)>,
    repeats: usize,
) -> Platform {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_string(&engine, &clock, source).unwrap();
    let dispatcher = Rc::new(SingleTask {
        task,
        next_task: None,
        active: Cell::new(false),
        completed: Cell::new(0),
        repeats,
        changed: Repeated::new(()),
    });
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
            assert!(!dispatcher.active.get());
            assert_eq!(platform.pe("pe0").unwrap().memory_traffic_bytes(), (0, 0));
            assert_eq!(platform.pe("pe0").unwrap().peak_sram_bytes(), 0);
            assert!(
                platform
                    .pe("pe0")
                    .unwrap()
                    .machine_ops()
                    .by_operation
                    .is_empty()
            );
            for name in platform.memory_names() {
                let memory = platform.memory(&name).unwrap();
                assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
            }
        }
    }
    platform
}

fn cached(source: &str, line: usize) -> String {
    source.replace(
        " - {connect: [pe.pe0, 'fabric.f@(0,0)']}",
        " - {connect: [pe.pe0, cache.c.dev]}\n - {connect: [cache.c.mem, 'fabric.f@(0,0)']}",
    ) + &format!(
        "caches:\n - name: c\n   config: {{line_size_bytes: {line}, num_sets: 16, num_ways: 1, bw_bytes_per_tick: 32, delay_ticks: 1}}\n"
    )
}

#[test]
fn cache_line_aligned_devices_preserve_payload_then_real_hits() {
    let input = TensorView::new_full(Tensor::new(&[8], &DataType::Int8, 0).unwrap());
    let output = TensorView::new_full(Tensor::new(&[8], &DataType::Int8, 64).unwrap());
    let platform = execute_repeated(
        &cached(&source(4, 4, 4), 4),
        Task::ProgramTask {
            config: Box::new(copy(input, output, "[i]")),
        },
        Some((16, 16)),
        2,
    );
    assert_eq!(platform.memory("m0").unwrap().bytes_read(), 4);
    assert_eq!(platform.memory("m1").unwrap().bytes_read(), 4);
    let cache = platform.cache("c").unwrap().traffic_snapshot();
    assert_eq!(
        (
            cache.request_misses,
            cache.request_hits,
            cache.fill_requests
        ),
        (4, 12, 2)
    );
}

#[test]
fn sparse_cache_reads_round_only_touched_lines_and_excluded_views_never_lookup() {
    let input = TensorView::new(
        Tensor::new(&[2, 8], &DataType::Int8, 0).unwrap(),
        &[2, 1],
        &[0, 0],
    )
    .unwrap();
    let output = TensorView::new_full(Tensor::new(&[2], &DataType::Int8, 64).unwrap());
    let platform = execute(
        &cached(&source(16, 32, 4), 4),
        copy(input, output, "[i, 0]"),
        Some((2, 2)),
    );
    assert_eq!(platform.memory("m0").unwrap().bytes_read(), 8);
    let cache = platform.cache("c").unwrap().traffic_snapshot();
    assert_eq!((cache.request_misses, cache.fill_requests), (2, 2));
    let config = cached(
        &source(16, 32, 4).replacen("[m0, m1, out]", "[m0, out]", 1),
        4,
    );
    let remote = TensorView::new_full(Tensor::new(&[2], &DataType::Int8, 32).unwrap());
    let output = TensorView::new_full(Tensor::new(&[2], &DataType::Int8, 64).unwrap());
    let platform = execute(&config, copy(remote, output, "[i]"), None);
    let cache = platform.cache("c").unwrap().traffic_snapshot();
    assert_eq!(
        (
            cache.request_misses,
            cache.request_hits,
            cache.fill_requests
        ),
        (0, 0, 0)
    );
}

#[test]
fn cache_rejects_physical_boundaries_not_aligned_to_whole_lines() {
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let error = Platform::from_string(&engine, &clock, &cached(&source(2, 2, 2), 4))
        .err()
        .expect("unaligned cache memory must fail platform validation");
    assert!(error.to_string().contains("4-byte fetch size"), "{error}");
}

#[test]
fn bound_scale_metadata_is_checked_before_active_even_when_not_in_arithmetic() {
    let source = separate_dma_source();
    let view = |addr, dtype| TensorView::new_full(Tensor::new(&[2], &dtype, addr).unwrap());
    let mut task = copy(view(0, DataType::Int8), view(64, DataType::Int8), "[i]");
    task.program.inputs.push("Scale".into());
    task.program.metadata_inputs.push("Scale".into());
    task.inputs.push(view(2, DataType::Uint8));
    execute(&source, task, None);
}

#[test]
fn cache_unfetched_sector_remains_a_real_later_miss() {
    let first_input = TensorView::new(
        Tensor::new(&[2, 8], &DataType::Int8, 0).unwrap(),
        &[2, 1],
        &[0, 0],
    )
    .unwrap();
    let first_output = TensorView::new_full(Tensor::new(&[2], &DataType::Int8, 64).unwrap());
    let later_input = TensorView::new_full(Tensor::new(&[1], &DataType::Int8, 4).unwrap());
    let later_output = TensorView::new_full(Tensor::new(&[1], &DataType::Int8, 68).unwrap());
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_string(&engine, &clock, &cached(&source(16, 32, 4), 4)).unwrap();
    let dispatcher = Rc::new(SingleTask {
        task: Task::ProgramTask {
            config: Box::new(copy(first_input, first_output, "[i, 0]")),
        },
        next_task: Some(Task::ProgramTask {
            config: Box::new(copy(later_input, later_output, "[i]")),
        }),
        active: Cell::new(false),
        completed: Cell::new(0),
        repeats: 2,
        changed: Repeated::new(()),
    });
    let interface: Rc<dyn Dispatch> = dispatcher.clone();
    platform.attach_dispatcher(&interface);
    engine.run().unwrap();
    assert_eq!(dispatcher.completed.get(), 2);
    assert_eq!(platform.pe("pe0").unwrap().memory_traffic_bytes(), (3, 3));
    assert_eq!(platform.memory("m0").unwrap().bytes_read(), 12);
    let cache = platform.cache("c").unwrap().traffic_snapshot();
    assert_eq!(
        (
            cache.request_misses,
            cache.request_hits,
            cache.fill_requests
        ),
        (3, 0, 3)
    );
}

#[test]
fn strided_islands_allow_untouched_holes_but_missing_bytes_reject_before_active() {
    let input = TensorView::new(
        Tensor::new(&[2, 4], &DataType::Int8, 0).unwrap(),
        &[2, 1],
        &[0, 0],
    )
    .unwrap();
    let output = TensorView::new_full(Tensor::new(&[2], &DataType::Int8, 64).unwrap());
    execute(
        &source(1, 4, 1),
        copy(input.clone(), output.clone(), "[i, 0]"),
        Some((2, 2)),
    );
    execute(&source(1, 5, 1), copy(input, output, "[i, 0]"), None);
}

#[test]
fn contiguous_view_crossing_devices_requires_region_bounded_requests() {
    let input = TensorView::new_full(Tensor::new(&[4], &DataType::Int8, 0).unwrap());
    let output = TensorView::new_full(Tensor::new(&[4], &DataType::Int8, 64).unwrap());
    let platform = execute(&source(2, 2, 2), copy(input, output, "[i]"), Some((4, 4)));
    assert_eq!(platform.memory("m0").unwrap().bytes_read(), 2);
    assert_eq!(platform.memory("m1").unwrap().bytes_read(), 2);
}

fn separate_dma_source() -> String {
    source(2, 2, 2)
        .replace(
            "operation_classes:",
            " - name: global\n   devices: [m0, m1, out]\noperation_classes:",
        )
        .replacen("[m0, m1, out]", "[m0, out]", 1)
        .replace(
            "memory_map: local",
            "memory_map: local\n   dma_memory_map: global",
        )
        .replace("columns: 4", "columns: 5")
        + " - {connect: [pe.pe0.dma, 'fabric.f@(4,0)']}\n"
}

#[test]
fn connected_dma_uses_global_map_while_compute_remote_inputs_and_outputs_reject() {
    let source = separate_dma_source();
    let view = |addr| TensorView::new_full(Tensor::new(&[2], &DataType::Int8, addr).unwrap());
    execute(&source, copy(view(2), view(64), "[i]"), None);
    execute(&source, copy(view(0), view(2), "[i]"), None);
    let platform = execute_task(
        &source,
        Task::TransferTask {
            config: Box::new(gwr_models::transfer::TransferTaskConfig {
                id: "explicit-stage".into(),
                source: view(2),
                destination: view(64),
                buffer_bytes: 2,
            }),
        },
        Some((2, 2)),
    );
    assert_eq!(platform.memory("m1").unwrap().bytes_read(), 2);
    assert_eq!(platform.memory("out").unwrap().bytes_written(), 2);
    for local_dma in [
        source.replace("   dma_memory_map: global\n", ""),
        source.replace("dma_memory_map: global", "dma_memory_map: local"),
    ] {
        execute_task(
            &local_dma,
            Task::TransferTask {
                config: Box::new(gwr_models::transfer::TransferTaskConfig {
                    id: "local-dma-reject".into(),
                    source: view(2),
                    destination: view(64),
                    buffer_bytes: 2,
                }),
            },
            None,
        );
    }
    for rejected in [
        source.replace("dma_memory_map: global", "dma_memory_map: missing"),
        source.replace(" - {connect: [pe.pe0.dma, 'fabric.f@(4,0)']}\n", ""),
    ] {
        let parsed: PlatformConfig = yaml_serde::from_str(&rejected).unwrap();
        assert!(parsed.validate().is_err());
        let mut engine = Engine::default();
        let clock = engine.default_clock();
        assert!(Platform::from_string(&engine, &clock, &rejected).is_err());
    }
}

#[test]
fn omitted_selector_and_absent_dma_keep_legacy_transfer_fallback() {
    let view = |addr| TensorView::new_full(Tensor::new(&[2], &DataType::Int8, addr).unwrap());
    execute_task(
        &source(2, 2, 2),
        Task::TransferTask {
            config: Box::new(gwr_models::transfer::TransferTaskConfig {
                id: "legacy-fallback".into(),
                source: view(2),
                destination: view(64),
                buffer_bytes: 2,
            }),
        },
        Some((2, 2)),
    );
    // A zero-sized tensor/view is not valid in the current Tensor API.
    assert!(Tensor::new(&[0], &DataType::Int8, 0).is_err());
    assert!(TensorView::new(Tensor::new(&[2], &DataType::Int8, 0).unwrap(), &[0], &[0]).is_err());
    assert_eq!(Tensor::new(&[], &DataType::Int8, 0).unwrap().num_bytes(), 1);
}

#[test]
fn nonzero_strided_and_packed_offsets_validate_only_actual_touched_islands() {
    for (dtype, width, offset, output_bytes) in
        [(DataType::Int8, 4, 1, 2), (DataType::Fp4, 8, 3, 1)]
    {
        let input = TensorView::new(
            Tensor::new(&[2, width], &dtype, 0).unwrap(),
            &[2, 1],
            &[0, offset],
        )
        .unwrap();
        let output = TensorView::new_full(Tensor::new(&[2], &dtype, 64).unwrap());
        // Both views touch physical bytes1/5, including odd nibble offsets.
        let mapped = source(1, 5, 1).replace("base_address: 0,", "base_address: 1,");
        execute(
            &mapped,
            copy(input.clone(), output.clone(), "[i, 0]"),
            Some((2, output_bytes)),
        );
        let missing = source(1, 6, 1).replace("base_address: 0,", "base_address: 1,");
        execute(&missing, copy(input, output, "[i, 0]"), None);
    }
}

#[test]
fn packed_views_count_unique_physical_bytes_and_last_address_is_mapped() {
    let view = TensorView::new(
        Tensor::new(&[2, 8], &DataType::Fp4, 0).unwrap(),
        &[2, 1],
        &[0, 0],
    )
    .unwrap();
    let output = TensorView::new_full(Tensor::new(&[2], &DataType::Fp4, 64).unwrap());
    execute(&source(1, 4, 1), copy(view, output, "[i, 0]"), Some((2, 1)));
    let input = TensorView::new_full(Tensor::new(&[1], &DataType::Int8, u64::MAX).unwrap());
    let output = TensorView::new_full(Tensor::new(&[1], &DataType::Int8, 64).unwrap());
    let platform = execute(
        &source(1, u64::MAX, 1),
        copy(input, output, "[i]"),
        Some((1, 1)),
    );
    assert_eq!(platform.memory("m1").unwrap().bytes_read(), 1);
}

#[test]
fn independent_dma_map_is_an_optional_strict_named_selector() {
    let source = "memory_maps: []\nprocessing_elements:\n - name: pe0\n   memory_map: local\n   dma_memory_map: global\n   config: {}\n";
    let parsed: PlatformConfig = yaml_serde::from_str(source).unwrap();
    let emitted = platform_to_yaml_str(&parsed).unwrap();
    assert!(emitted.contains("dma_memory_map: global"));
    for invalid in ["42", "42.0", "true", "[global]", "{name: global}"] {
        assert!(
            yaml_serde::from_str::<PlatformConfig>(&source.replace("global", invalid)).is_err()
        );
    }
    let omitted: PlatformConfig =
        yaml_serde::from_str(&source.replace("   dma_memory_map: global\n", "")).unwrap();
    assert!(
        !platform_to_yaml_str(&omitted)
            .unwrap()
            .contains("dma_memory_map")
    );
}
