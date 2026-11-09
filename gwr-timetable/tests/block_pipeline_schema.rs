// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn compute_pipeline_roundtrips_and_reaches_graph() {
    let source = include_str!("../examples/fused_region.yaml").replace(
        "    tiling:",
        "    pipeline: {input_buffers: 3, output_buffers: 2, prefetch_distance: 2}\n    tiling:",
    );
    let file = TimetableFile::from_string(&source).unwrap();
    let encoded = yaml_serde::to_string(&file).unwrap();
    assert!(encoded.contains("input_buffers: 3"));
    let graph = TimetableFile::from_string(&encoded)
        .unwrap()
        .into_graph()
        .unwrap();
    let pipeline = graph
        .nodes()
        .iter()
        .find_map(|node| node.pipeline())
        .unwrap();
    assert_eq!(
        (
            pipeline.input_buffers,
            pipeline.output_buffers,
            pipeline.prefetch_distance
        ),
        (3, 2, 2)
    );
    assert_eq!(
        pipeline.mode,
        gwr_models::processing_element::program::pipeline::PipelineMode::Overlap
    );
}

#[test]
fn pipeline_request_rejects_zero_buffers_unknown_fields_and_excess_prefetch() {
    for request in [
        "{input_buffers: 0, output_buffers: 1, prefetch_distance: 0}",
        "{input_buffers: 1, output_buffers: 0, prefetch_distance: 0}",
        "{input_buffers: 2, output_buffers: 1, prefetch_distance: 2}",
        "{input_buffers: 2, output_buffers: 1, prefetch_distance: 1, unknown: 1}",
        "{input_buffers: 2, output_buffers: 1}",
    ] {
        let source = include_str!("../examples/fused_region.yaml").replace(
            "    tiling:",
            &format!("    pipeline: {request}\n    tiling:"),
        );
        assert!(
            TimetableFile::from_string(&source)
                .and_then(TimetableFile::into_graph)
                .is_err(),
            "{request}"
        );
    }
    for pipeline in ["", "    pipeline: null\n"] {
        let source = include_str!("../examples/fused_region.yaml")
            .replace("    tiling:", &format!("{pipeline}    tiling:"));
        let graph = TimetableFile::from_string(&source)
            .unwrap()
            .into_graph()
            .unwrap();
        assert!(graph.nodes().iter().all(|node| node.pipeline().is_none()));
    }
}

#[test]
fn compute_pipeline_reaches_task_config() {
    use std::rc::Rc;

    use gwr_models::processing_element::dispatch::Dispatch;
    use gwr_models::processing_element::task::Task;
    let mut engine = gwr_engine::engine::Engine::default();
    let clock = engine.default_clock();
    let platform = Rc::new(gwr_platform::Platform::from_string(&engine, &clock, r"
operation_classes:
  matrix: {operations: [matrix_mac], arity: 2}
  convert: {operations: [convert], arity: 1}
memory_maps: [{name: mm, devices: []}]
processing_elements:
- name: pe0
  memory_map: mm
  config:
    sram_bytes: 4096
    block_execution: {max_compute_tiles: 1, max_load_chunks: 1, max_store_tiles: 1, transfer_port: lsu, accumulator_storage: shared_sram}
    compute:
      engines:
        matrix:
          matrix_accumulator_capacity_bytes: 4096
          capabilities: [{class: matrix, input_dtypes: [[fp8, fp8]], ops_per_tick: 16, execution: {instruction: tensor, accumulator: fp32}}]
        vector:
          capabilities: [{class: convert, input_dtypes: [[fp32]], ops_per_tick: 16}]
").unwrap());
    let file = TimetableFile::from_string(
        r"
nodes:
- {id: A, kind: tensor, config: {addr: 0, dtype: fp8, shape: [1, 2, 2]}}
- {id: B, kind: tensor, config: {addr: 64, dtype: fp8, shape: [1, 2, 2]}}
- id: gemm
  kind: compute
  pe: pe0
  pipeline: {mode: serial, input_buffers: 3, output_buffers: 2, prefetch_distance: 2}
  tiling: {sizes: {batch: 1, m: 2, n: 2, k: 1}}
  program:
    inputs: [a, b]
    outputs: [c]
    temporaries: {acc: {dtype: fp32}}
    assignments:
    - output: acc
      axes: [batch, m, n]
      expr:
        matrix_mac:
          over: [k]
          execution: {instruction: tensor, accumulator: fp32}
          args:
          - {read: {tensor: a, indices: [batch, m, k]}}
          - {read: {tensor: b, indices: [batch, k, n]}}
    - output: c
      axes: [batch, m, n]
      expr: {convert: [{read: {tensor: acc, indices: [batch, m, n]}}]}
- {id: C, kind: tensor, config: {addr: 128, dtype: fp16, shape: [1, 2, 2]}}
edges:
- {from: A, to: gemm.0, kind: data}
- {from: B, to: gemm.1, kind: data}
- {from: gemm.0, to: C, kind: data}
",
    )
    .unwrap();
    let graph = file.into_graph().unwrap();
    let index = graph
        .nodes()
        .iter()
        .position(|node| node.id() == "gemm")
        .unwrap();
    let expected = graph.nodes()[index].pipeline().cloned();
    let timetable = gwr_timetable::Timetable::new(engine.top(), graph, &platform).unwrap();
    let Task::ProgramTask { config } = timetable.task_by_id(index).unwrap() else {
        panic!("expected program task")
    };
    assert_eq!(config.pipeline, expected);
}
