// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::syntax::ExprSpec;
use gwr_models::processing_element::tensors::HasShape;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn one_named_program_validates_with_distinct_node_tiling() {
    let file = TimetableFile::from_string(SHARED_ADD).unwrap();
    assert_eq!(file.programs.len(), 1);
    file.validate().unwrap();
    let serialized = yaml_serde::to_string(&file).unwrap();
    assert_eq!(serialized.matches("program: vector_add").count(), 2);
    TimetableFile::from_string(&serialized)
        .unwrap()
        .validate()
        .unwrap();
}

#[test]
fn omitted_view_lists_cover_every_resolved_program_port() {
    let graph = TimetableFile::from_string(SHARED_ADD)
        .unwrap()
        .into_graph()
        .unwrap();
    for (id, shape) in [("add4", 4), ("add8", 8)] {
        let index = graph
            .nodes()
            .iter()
            .position(|node| node.id() == id)
            .unwrap();
        let views = graph.compute_views(index).unwrap();
        assert_eq!(views.inputs().len(), 2);
        assert_eq!(views.outputs().len(), 1);
        for view in views.inputs().iter().chain(views.outputs()) {
            let view = view.as_ref().unwrap();
            assert_eq!(view.shape().dims(), &[shape]);
            assert_eq!(view.offsets(), &[0]);
        }
    }
}

#[test]
fn either_view_list_can_be_omitted_independently() {
    let source = SHARED_ADD
        .replace(
            "    tiling: { sizes: { i: 2 } }\n",
            "    tiling: { sizes: { i: 2 } }\n    output_views: [{ offsets: [0], shape: [4] }]\n",
        )
        .replace(
            "    pe: pe1\n    program: vector_add\n",
            "    pe: pe1\n    program: vector_add\n    input_views: [{ offsets: [0], shape: [8] }, { offsets: [0], shape: [8] }]\n",
        );
    let graph = TimetableFile::from_string(&source)
        .unwrap()
        .into_graph()
        .unwrap();
    for id in ["add4", "add8"] {
        let index = graph
            .nodes()
            .iter()
            .position(|node| node.id() == id)
            .unwrap();
        let views = graph.compute_views(index).unwrap();
        assert_eq!(views.inputs().len(), 2);
        assert_eq!(views.outputs().len(), 1);
        assert!(views.inputs().iter().all(Option::is_some));
        assert!(views.outputs().iter().all(Option::is_some));
    }
}

#[test]
fn explicitly_empty_view_list_does_not_acquire_implicit_ports() {
    let source = SHARED_ADD.replace(
        "    tiling: { sizes: { i: 2 } }\n",
        "    tiling: { sizes: { i: 2 } }\n    input_views: []\n",
    );
    let error = TimetableFile::from_string(&source)
        .unwrap()
        .into_graph()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("input tensor index 0 is out of range"),
        "{error}"
    );
}

#[test]
fn indexed_reduction_call_rebinds_source_read() {
    let source = r"
programs:
  pool:
    inputs: [A]
    outputs: [Y, I]
    extents: { o: 2, k: 2 }
    assignments:
      - output: Y
        indices_output: I
        axes: [o]
        expr: { reduce: { op: max, over: [k], value: { read: { tensor: A, indices: [{ affine: { terms: { o: 2, k: 1 }, offset: 0 } }], bounds: exclude } } } }
  wrapper:
    inputs: [X]
    outputs: [V, J]
    calls:
      - { program: pool, inputs: { A: X }, outputs: { Y: V, I: J } }
nodes:
  - { id: x, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - id: run
    kind: compute
    pe: pe0
    program: wrapper
  - { id: v, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [2] } }
  - { id: j, kind: tensor, config: { addr: 0x3000, dtype: int64, shape: [2] } }
edges:
  - { from: x, to: run.0, kind: data }
  - { from: run.0, to: v, kind: data }
  - { from: run.1, to: j, kind: data }
";
    let graph = TimetableFile::from_string(source)
        .unwrap()
        .into_graph()
        .unwrap();
    let program = graph
        .nodes()
        .iter()
        .find_map(|node| node.program())
        .unwrap();
    let ExprSpec::Reduce { value, .. } = &program.assignments[0].expr else {
        panic!("expected indexed reduction")
    };
    let ExprSpec::Read { tensor, .. } = &**value else {
        panic!("expected read")
    };
    assert_eq!(tensor, "X");
    assert_eq!(program.assignments[0].indices_output.as_deref(), Some("J"));
}

#[test]
fn missing_program_reference_names_the_node_and_program() {
    let source = SHARED_ADD.replacen("program: vector_add", "program: missing", 1);
    let error = TimetableFile::from_string(&source)
        .unwrap()
        .validate()
        .unwrap_err()
        .to_string();
    assert!(error.contains("add4"), "{error}");
    assert!(error.contains("missing"), "{error}");
}

const SHARED_ADD: &str = r"
programs:
  vector_add:
    inputs: [A, B]
    outputs: [Y]
    assignments:
      - output: Y
        axes: [i]
        expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
nodes:
  - { id: a4, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - { id: b4, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
  - { id: y4, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4] } }
  - id: add4
    kind: compute
    pe: pe0
    program: vector_add
    tiling: { sizes: { i: 2 } }
  - { id: a8, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [8] } }
  - { id: b8, kind: tensor, config: { addr: 0x5000, dtype: fp32, shape: [8] } }
  - { id: y8, kind: tensor, config: { addr: 0x6000, dtype: fp32, shape: [8] } }
  - id: add8
    kind: compute
    pe: pe1
    program: vector_add
edges:
  - { from: a4, to: add4.0, kind: data }
  - { from: b4, to: add4.1, kind: data }
  - { from: add4, to: y4, kind: data }
  - { from: a8, to: add8.0, kind: data }
  - { from: b8, to: add8.1, kind: data }
  - { from: add8, to: y8, kind: data }
";

#[test]
fn nested_calls_expand_to_one_tiled_program() {
    let file = TimetableFile::from_string(COMPOSED).unwrap();
    let serialized = yaml_serde::to_string(&file).unwrap();
    assert!(serialized.contains("program: add_pair"));
    TimetableFile::from_string(&serialized)
        .unwrap()
        .validate()
        .unwrap();
    let graph = file.into_graph().unwrap();
    let program = graph
        .nodes()
        .iter()
        .find_map(|node| node.program())
        .unwrap();
    assert_eq!(program.assignments.len(), 3);
    assert!(
        program
            .assignments
            .iter()
            .all(|assignment| assignment.axes == ["j"])
    );
    assert_eq!(program.assignments[0].output.as_deref(), Some("T"));
    assert!(program.temporaries.contains_key("__call0_inner"));
    assert_eq!(program.assignments[2].output.as_deref(), Some("Y"));
}

#[test]
fn cycles_and_invalid_bindings_report_the_call_path() {
    let cycle = COMPOSED.replace("program: add_pair", "program: add_then_mul");
    let error = TimetableFile::from_string(&cycle)
        .unwrap()
        .validate()
        .unwrap_err()
        .to_string();
    assert!(error.contains("cycle"), "{error}");
    assert!(error.contains("add_then_mul"), "{error}");

    let invalid = COMPOSED.replacen("inputs: { A: A, B: B }", "inputs: { A: A }", 1);
    let error = TimetableFile::from_string(&invalid)
        .unwrap()
        .validate()
        .unwrap_err()
        .to_string();
    assert!(error.contains("add_pair"), "{error}");
    assert!(error.contains('B'), "{error}");

    let malformed = COMPOSED.replacen("tensor: A, indices", "tensor: missing, indices", 1);
    let error = TimetableFile::from_string(&malformed)
        .unwrap()
        .validate()
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing"), "{error}");
    assert!(error.contains("add"), "{error}");
}

#[test]
fn repeated_calls_keep_their_internal_tensors_separate() {
    let source = COMPOSED.replace("program: mul", "program: add_pair");
    let graph = TimetableFile::from_string(&source)
        .unwrap()
        .into_graph()
        .unwrap();
    let program = graph
        .nodes()
        .iter()
        .find_map(|node| node.program())
        .unwrap();
    assert_eq!(program.assignments.len(), 4);
    assert!(program.temporaries.contains_key("__call0_inner"));
    assert!(program.temporaries.contains_key("__call1_inner"));
}

#[test]
fn composed_program_executes_as_one_tiled_compute_node() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker;
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(
        Platform::from_string(
            &engine,
            &clock,
            include_str!("../examples/compute_region_platform.yaml"),
        )
        .unwrap(),
    );
    let graph = TimetableFile::from_string(COMPOSED)
        .unwrap()
        .into_graph()
        .unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let ops = platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(ops.by_operation.get("add"), Some(&4));
    assert_eq!(ops.by_operation.get("mul"), Some(&4));
}

const COMPOSED: &str = r"
programs:
  add:
    inputs: [A, B]
    outputs: [Y]
    assignments:
      - output: Y
        axes: [i]
        expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
  mul:
    inputs: [A, B]
    outputs: [Y]
    assignments:
      - output: Y
        axes: [i]
        expr: { mul: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
  add_pair:
    inputs: [A, B]
    outputs: [Y]
    temporaries: { inner: { dtype: fp32 } }
    assignments:
      - output: Y
        axes: [i]
        expr: { read: { tensor: inner, indices: [i] } }
    calls:
      - program: add
        inputs: { A: A, B: B }
        outputs: { Y: inner }
  add_then_mul:
    inputs: [A, B, C]
    outputs: [Y]
    temporaries: { T: { dtype: fp32 } }
    calls:
      - program: add_pair
        inputs: { A: A, B: B }
        outputs: { Y: T }
        axes: { i: j }
      - program: mul
        inputs: { A: T, B: C }
        outputs: { Y: Y }
        axes: { i: j }
nodes:
  - { id: a, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - { id: b, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
  - { id: c, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4] } }
  - { id: y, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [4] } }
  - id: compute
    kind: compute
    pe: pe0
    program: add_then_mul
    tiling: { sizes: { j: 2 } }
edges:
  - { from: a, to: compute.0, kind: data }
  - { from: b, to: compute.1, kind: data }
  - { from: c, to: compute.2, kind: data }
  - { from: compute, to: y, kind: data }
";

#[test]
fn call_renames_reduction_axes_for_a_matrix_program() {
    let source = r"
programs:
  gemm:
    inputs: [A, B]
    outputs: [Y]
    assignments:
      - output: Y
        axes: [m, n]
        expr: { matrix_mac: { over: [k], args: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] } }
  projection:
    inputs: [X, W]
    outputs: [Z]
    calls:
      - program: gemm
        inputs: { A: X, B: W }
        outputs: { Y: Z }
        axes: { m: rows, n: columns, k: channels }
nodes:
  - { id: x, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [2, 3] } }
  - { id: w, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [3, 2] } }
  - { id: z, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [2, 2] } }
  - id: projection_node
    kind: compute
    pe: pe0
    program: projection
    tiling: { sizes: { rows: 1, columns: 1, channels: 3 } }
edges:
  - { from: x, to: projection_node.0, kind: data }
  - { from: w, to: projection_node.1, kind: data }
  - { from: projection_node, to: z, kind: data }
";
    let graph = TimetableFile::from_string(source)
        .unwrap()
        .into_graph()
        .unwrap();
    let program = graph
        .nodes()
        .iter()
        .find_map(|node| node.program())
        .unwrap();
    assert_eq!(program.assignments[0].axes, ["rows", "columns"]);
    let ExprSpec::MatrixMac { over, .. } = &program.assignments[0].expr else {
        panic!("expected matrix MAC");
    };
    assert_eq!(over, &["channels"]);
}
