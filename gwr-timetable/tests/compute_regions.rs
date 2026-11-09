// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::syntax::{ExprSpec, IndexSpec, ProgramSpec, TileSpec};
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::{NodeSection, TimetableFile};

#[test]
fn fused_temporary_reduction_runs_without_materializing_the_intermediate() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: mul", "class: compare");
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_source).unwrap());
    let file = TimetableFile::from_string(r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [2, 3] } }
  - id: fused
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      temporaries: { T: { dtype: fp32 } }
      assignments:
        - output: T
          axes: [o, k]
          expr: { add: [{ read: { tensor: A, indices: [o, k] } }, { read: { tensor: A, indices: [o, k] } }] }
        - output: Y
          axes: [o]
          expr: { reduce: { op: max, over: [k], value: { read: { tensor: T, indices: [o, k] } } } }
    tiling: { sizes: { o: 1 } }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [2] } }
edges:
  - { from: A, to: fused.0, kind: data }
  - { from: fused.0, to: Y, kind: data }
").unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    assert_eq!(
        platform.pe("pe0").unwrap().machine_ops().by_operation["compare"],
        4
    );
    assert_eq!(platform.pe("pe0").unwrap().memory_traffic_bytes(), (24, 8));
}

#[test]
fn affine_sum_reduction_transfers_only_valid_window_elements() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
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
    let file = TimetableFile::from_string(r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [3] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [3] } }
  - id: window
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      extents: { y: 3, k: 3 }
      assignments:
        - output: Y
          axes: [y]
          expr: { reduce: { op: sum, over: [k], value: { mul: [{ read: { tensor: A, indices: [{ affine: { terms: { y: 1, k: 1 }, offset: -1 } }], bounds: exclude } }, { read: { tensor: B, indices: [k] } }] } } }
    tiling: { sizes: { y: 1 } }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [3] } }
edges:
  - { from: A, to: window.0, kind: data }
  - { from: B, to: window.1, kind: data }
  - { from: window.0, to: Y, kind: data }
").unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    assert_eq!(platform.pe("pe0").unwrap().memory_traffic_bytes(), (64, 12));
}

#[test]
fn dependency_only_input_is_not_loaded() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
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
    let file = TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - { id: dependency, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
  - id: copy
    kind: compute
    pe: pe0
    program:
      inputs: [A, dependency]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr: { read: { tensor: A, indices: [i] } }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4] } }
edges:
  - { from: A, to: copy.0, kind: data }
  - { from: dependency, to: copy.1, kind: data }
  - { from: copy.0, to: Y, kind: data }
",
    )
    .unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    assert_eq!(platform.pe("pe0").unwrap().memory_traffic_bytes(), (16, 16));
    assert_eq!(platform.pe("pe0").unwrap().peak_sram_bytes(), 32);
}

#[test]
fn independent_nodes_use_replicated_vector_engines() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: mul", "class: add");
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_source).unwrap());
    let file = TimetableFile::from_string(r"
programs:
  add_pair:
    inputs: [A, B]
    outputs: [Y]
    assignments:
      - output: Y
        axes: [i]
        expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
  - { id: first, kind: compute, pe: pe0, program: add_pair }
  - { id: second, kind: compute, pe: pe0, program: add_pair }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4] } }
  - { id: Z, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [4] } }
edges:
  - { from: A, to: first.0, kind: data }
  - { from: B, to: first.1, kind: data }
  - { from: first.0, to: Y, kind: data }
  - { from: A, to: second.0, kind: data }
  - { from: B, to: second.1, kind: data }
  - { from: second.0, to: Z, kind: data }
").unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let events = tracker.events();
    assert!(
        events
            .iter()
            .any(|event| event.contains("lane::compute::vector_add::0"))
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("lane::compute::vector_mul::0"))
    );
    assert_eq!(
        platform.pe("pe0").unwrap().machine_ops().by_operation["add"],
        8
    );
}

#[test]
fn parses_a_fused_tiled_program_without_an_operator_tag() {
    let source = r"
inputs: [A, B, C]
outputs: [Y]
temporaries: { T: { dtype: fp32 } }
assignments:
  - output: T
    axes: [i]
    expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
  - output: Y
    axes: [i]
    expr: { mul: [{ read: { tensor: T, indices: [i] } }, { read: { tensor: C, indices: [i] } }] }
";
    let program: ProgramSpec = yaml_serde::from_str(source).unwrap();
    assert_eq!(program.inputs, ["A", "B", "C"]);
    assert_eq!(program.outputs, ["Y"]);
    assert_eq!(program.assignments.len(), 2);
    assert!(matches!(&program.assignments[0].expr, ExprSpec::Call { op, .. } if op == "add"));
    assert!(matches!(&program.assignments[1].expr, ExprSpec::Call { op, .. } if op == "mul"));
    let round_trip: ProgramSpec =
        yaml_serde::from_str(&yaml_serde::to_string(&program).unwrap()).unwrap();
    assert_eq!(round_trip, program);
}

#[test]
fn parses_affine_indices_and_optional_tiling() {
    let source = r"
inputs: [A]
outputs: [Y]
extents: { oh: 3, kh: 3 }
assignments:
  - output: Y
    axes: [oh]
    expr:
      reduce:
        op: max
        over: [kh]
        value:
          read:
            tensor: A
            indices: [{ affine: { terms: { oh: 2, kh: 1 }, offset: -1 } }]
            bounds: exclude
";
    let program: ProgramSpec = yaml_serde::from_str(source).unwrap();
    let ExprSpec::Reduce { value, .. } = &program.assignments[0].expr else {
        panic!("expected max");
    };
    let ExprSpec::Read { indices, .. } = &**value else {
        panic!("expected read");
    };
    assert!(matches!(indices[0], IndexSpec::Affine { .. }));
    let tile: TileSpec = yaml_serde::from_str("sizes: { oh: 2 }").unwrap();
    assert_eq!(tile.sizes["oh"], 2);
}

#[test]
fn rejects_ambiguous_or_incomplete_expressions() {
    for expression in [
        "{ add: { wrong: [] } }",
        "{ read: { tensor: A, indices: [i] }, mul: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: A, indices: [i] } }] }",
        "{ reduce: { op: sum, over: [i] } }",
    ] {
        let source = format!(
            "inputs: [A]\noutputs: [Y]\nassignments:\n  - output: Y\n    axes: [i]\n    expr: {expression}\n"
        );
        assert!(
            yaml_serde::from_str::<ProgramSpec>(&source).is_err(),
            "accepted {expression}"
        );
    }
}

#[test]
fn timetable_compute_node_carries_its_program_and_tiling() {
    let timetable = TimetableFile::from_string(r"
nodes:
  - id: A
    kind: tensor
    config: { addr: 0x1000, dtype: fp32, shape: [8] }
  - id: B
    kind: tensor
    config: { addr: 0x2000, dtype: fp32, shape: [8] }
  - id: Y
    kind: tensor
    config: { addr: 0x3000, dtype: fp32, shape: [8] }
  - id: add
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
    tiling: { sizes: { i: 4 } }
edges:
  - { from: A, to: add.0, kind: data }
  - { from: B, to: add.1, kind: data }
  - { from: add.0, to: Y, kind: data }
").unwrap();
    let NodeSection::Compute {
        program, tiling, ..
    } = &timetable.nodes[3]
    else {
        panic!("expected compute");
    };
    let gwr_timetable::timetable_file::ProgramSection::Inline(program) = program else {
        panic!("expected inline program");
    };
    assert_eq!(program.inputs, ["A", "B"]);
    assert_eq!(tiling.as_ref().unwrap().sizes["i"], 4);
}

#[test]
fn fused_timetable_executes_tiled_add_then_multiply() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
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
    let file = TimetableFile::from_string(r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [8] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [8] } }
  - { id: C, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [8] } }
  - id: fused
    kind: compute
    pe: pe0
    program:
      inputs: [A, B, C]
      outputs: [Y]
      temporaries: { T: { dtype: fp32 } }
      assignments:
        - output: T
          axes: [i]
          expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
        - output: Y
          axes: [i]
          expr: { mul: [{ read: { tensor: T, indices: [i] } }, { read: { tensor: C, indices: [i] } }] }
    tiling: { sizes: { i: 4 } }
  - { id: Y, kind: tensor, config: { addr: 0x5000, dtype: fp32, shape: [8] } }
edges:
  - { from: A, to: fused.0, kind: data }
  - { from: B, to: fused.1, kind: data }
  - { from: C, to: fused.2, kind: data }
  - { from: fused.0, to: Y, kind: data }
").unwrap();
    let graph = file.into_graph().unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let ops = platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(ops.by_operation.get("add"), Some(&8));
    assert_eq!(ops.by_operation.get("mul"), Some(&8));
}

#[test]
fn independent_assignments_overlap_on_separate_engines() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
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
    let file = TimetableFile::from_string(r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [8] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [8] } }
  - { id: C, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [8] } }
  - id: branches
    kind: compute
    pe: pe0
    program:
      inputs: [A, B, C]
      outputs: [Y, Z]
      assignments:
        - output: Y
          axes: [i]
          expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
        - output: Z
          axes: [i]
          expr: { mul: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: C, indices: [i] } }] }
  - { id: Y, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [8] } }
  - { id: Z, kind: tensor, config: { addr: 0x5000, dtype: fp32, shape: [8] } }
edges:
  - { from: A, to: branches.0, kind: data }
  - { from: B, to: branches.1, kind: data }
  - { from: C, to: branches.2, kind: data }
  - { from: branches.0, to: Y, kind: data }
  - { from: branches.1, to: Z, kind: data }
").unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let events = tracker.events();
    let position = |marker: &str| {
        events
            .iter()
            .position(|event| event.contains(marker))
            .unwrap_or_else(|| panic!("missing {marker}"))
    };
    assert!(position("assignment Z begin") < position("assignment Y end"));
    assert!(position("assignment Y begin") < position("assignment Z end"));
    // The third input read is still outstanding when the first assignment
    // starts; it does not consume that input.
    let c_read_activity_id = events
        .iter()
        .find(|event| event.contains("activity begin branches C read"))
        .unwrap()
        .split(':')
        .next()
        .unwrap();
    assert!(
        position("assignment Y begin") < position(&format!("{c_read_activity_id}: activity end"))
    );
}

#[test]
fn dependent_assignment_starts_before_unrelated_slow_branch_finishes() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_source = include_str!("../examples/compute_region_platform.yaml").replace(
        "class: mul, input_dtypes: [[fp32, fp32]], ops_per_tick: 1",
        "class: mul, input_dtypes: [[fp32, fp32]], ops_per_tick: 0.25",
    );
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_source).unwrap());
    let file = TimetableFile::from_string(r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [8] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [8] } }
  - id: fused
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y, Z]
      temporaries: { T: { dtype: fp32 } }
      assignments:
        - output: T
          axes: [i]
          expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
        - output: Z
          axes: [i]
          expr: { mul: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
        - output: Y
          axes: [i]
          expr: { add: [{ read: { tensor: T, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [8] } }
  - { id: Z, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [8] } }
edges:
  - { from: A, to: fused.0, kind: data }
  - { from: B, to: fused.1, kind: data }
  - { from: fused.0, to: Y, kind: data }
  - { from: fused.1, to: Z, kind: data }
").unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let events = tracker.events();
    let position = |marker: &str| {
        events
            .iter()
            .position(|event| event.contains(marker))
            .unwrap()
    };
    assert!(position("assignment Y begin") < position("assignment Z end"));
}

#[test]
fn tiled_sum_of_reads_and_additions_counts_each_operation() {
    for (reduction_body, expected_adds, uses_local_temporary) in [
        ("{ read: { tensor: A, indices: [i, k] } }", 8, false),
        (
            "{ add: [{ read: { tensor: A, indices: [i, k] } }, { read: { tensor: B, indices: [i, k] } }] }",
            20,
            false,
        ),
        ("{ read: { tensor: T, indices: [i, k] } }", 20, true),
    ] {
        let uses_b = uses_local_temporary || reduction_body.contains("tensor: B");
        let inputs = if uses_b { "[A, B]" } else { "[A]" };
        let b_edge = if uses_b {
            "  - { from: B, to: reduce.1, kind: data }"
        } else {
            ""
        };
        let temporary_decl = if uses_local_temporary {
            "      temporaries: { T: { dtype: fp32 } }"
        } else {
            ""
        };
        let prelude = if uses_local_temporary {
            "        - output: T\n          axes: [i, k]\n          expr: { add: [{ read: { tensor: A, indices: [i, k] } }, { read: { tensor: B, indices: [i, k] } }] }"
        } else {
            ""
        };
        let tiling = "{ i: 2, k: 2 }";
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
        let source = format!(
            r"
nodes:
  - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: fp32, shape: [4, 3] }} }}
  - {{ id: B, kind: tensor, config: {{ addr: 0x2000, dtype: fp32, shape: [4, 3] }} }}
  - id: reduce
    kind: compute
    pe: pe0
    program:
      inputs: {inputs}
      outputs: [Y]
{temporary_decl}
      assignments:
{prelude}
        - output: Y
          axes: [i]
          expr: {{ reduce: {{ op: sum, over: [k], value: {reduction_body} }} }}
    tiling: {{ sizes: {tiling} }}
  - {{ id: Y, kind: tensor, config: {{ addr: 0x3000, dtype: fp32, shape: [4] }} }}
edges:
  - {{ from: A, to: reduce.0, kind: data }}
{b_edge}
  - {{ from: reduce.0, to: Y, kind: data }}
"
        );
        let graph = TimetableFile::from_string(&source)
            .unwrap()
            .into_graph()
            .unwrap();
        let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
        let dispatcher: Rc<dyn Dispatch> = timetable.clone();
        platform.attach_dispatcher(&dispatcher);
        engine.run().unwrap();
        timetable.check_tasks_complete().unwrap();
        let ops = platform.pe("pe0").unwrap().machine_ops();
        assert_eq!(
            (
                ops.by_operation.get("add").copied().unwrap_or(0),
                ops.by_operation.get("mul").copied().unwrap_or(0)
            ),
            (expected_adds, 0),
            "{reduction_body}"
        );
    }
}

#[test]
fn nested_local_reductions_keep_full_intermediate_axis_in_sram() {
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
    let file = TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [2, 3, 4] } }
  - id: nested
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      temporaries: { T: { dtype: fp32 } }
      assignments:
        - output: T
          axes: [i, j]
          expr: { reduce: { op: sum, over: [k], value: { read: { tensor: A, indices: [i, j, k] } } } }
        - output: Y
          axes: [i]
          expr: { reduce: { op: sum, over: [j], value: { read: { tensor: T, indices: [i, j] } } } }
    tiling: { sizes: { i: 1, j: 2, k: 2 } }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [2] } }
edges:
  - { from: A, to: nested.0, kind: data }
  - { from: nested.0, to: Y, kind: data }
",
    )
    .unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    assert_eq!(
        platform.pe("pe0").unwrap().machine_ops().by_operation["add"],
        22
    );
    assert_eq!(platform.pe("pe0").unwrap().peak_sram_bytes(), 96);
}

#[test]
fn materialized_intermediate_waits_for_all_tiles_and_finishes_later() {
    let fused_source = include_str!("../examples/fused_region.yaml");
    let materialized_source = include_str!("../examples/materialized_region.yaml");
    let (fused_ticks, fused_events) = run_vector_timetable(fused_source);
    let (materialized_ticks, materialized_events) = run_vector_timetable(materialized_source);
    let external_transfer_bytes = |events: &[String]| -> usize {
        events
            .iter()
            .filter(|event| event.contains("created object") && event.contains("MemoryAccess:"))
            .map(|event| {
                let request = event.split("MemoryAccess: ").nth(1).unwrap();
                request
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .split('@')
                    .next()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
            })
            .sum()
    };
    assert_eq!(external_transfer_bytes(&fused_events), 128);
    assert_eq!(external_transfer_bytes(&materialized_events), 192);
    // Each 16-byte boundary request has an eight-tick memory response. The
    // LSU issues requests serially, while independent loads and engine work
    // overlap once their own dependencies are satisfied.
    assert_eq!((fused_ticks, materialized_ticks), (33, 58));
    let position = |events: &[String], marker: &str| {
        events
            .iter()
            .position(|event| event.contains(marker))
            .unwrap_or_else(|| panic!("missing {marker}"))
    };
    assert!(
        position(&fused_events, "fused tile {\"i\": 0..4} assignment Y begin")
            < position(&fused_events, "fused tile {\"i\": 4..8} assignment T end")
    );
    assert!(
        position(&materialized_events, "task3: completed")
            < position(
                &materialized_events,
                "multiply tile {\"i\": 0..4} assignment Y begin"
            )
    );
    assert!(
        fused_ticks < materialized_ticks,
        "fused={fused_ticks}, materialized={materialized_ticks}"
    );
}

fn run_vector_timetable(source: &str) -> (u64, Vec<String>) {
    run_vector_timetable_with_sram(source, 256)
}

#[test]
#[expect(clippy::too_many_lines)]
fn materialized_fanout_waits_for_tensor_completion_on_same_or_other_pes() {
    for same_pe in [false, true] {
        let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
            1000,
            gwr_track::log::Level::Trace,
        ));
        let tracking: gwr_track::Tracker = tracker;
        let mut engine = Engine::new(&tracking);
        let clock = engine.default_clock();
        let platform_source = include_str!("../examples/compute_region_platform.yaml")
            .replace("memories:\n", "  - name: pe1\n    memory_map: mm\n    config:\n      sram_bytes: 256\n      compute:\n        engines:\n          vector_add:\n            capabilities: [{ class: add, ops_per_tick: 1 }]\n          vector_mul:\n            capabilities: [{ class: mul, ops_per_tick: 1 }]\n  - name: pe2\n    memory_map: mm\n    config:\n      sram_bytes: 256\n      compute:\n        engines:\n          vector_add:\n            capabilities: [{ class: add, ops_per_tick: 1 }]\n          vector_mul:\n            capabilities: [{ class: mul, ops_per_tick: 1 }]\nmemories:\n");
        let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_source).unwrap());
        let source = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
  - id: produce
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [T]
      assignments:
        - output: T
          axes: [i]
          expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
    tiling: { sizes: { i: 2 } }
  - { id: T, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [4] } }
  - id: consume1
    kind: compute
    pe: pe1
    program:
      inputs: [T0, T1]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr: { add: [{ read: { tensor: T0, indices: [i] } }, { read: { tensor: T1, indices: [i] } }] }
  - { id: Y, kind: tensor, config: { addr: 0x4000, dtype: fp32, shape: [4] } }
  - id: consume2
    kind: compute
    pe: pe2
    program:
      inputs: [T0, T1]
      outputs: [Z]
      assignments:
        - output: Z
          axes: [i]
          expr: { add: [{ read: { tensor: T0, indices: [i] } }, { read: { tensor: T1, indices: [i] } }] }
  - { id: Z, kind: tensor, config: { addr: 0x5000, dtype: fp32, shape: [4] } }
edges:
  - { from: A, to: produce.0, kind: data }
  - { from: B, to: produce.1, kind: data }
  - { from: produce.0, to: T, kind: data }
  - { from: T, to: consume1.0, kind: data }
  - { from: T, to: consume1.1, kind: data }
  - { from: consume1.0, to: Y, kind: data }
  - { from: T, to: consume2.0, kind: data }
  - { from: T, to: consume2.1, kind: data }
  - { from: consume2.0, to: Z, kind: data }
";
        let source = if same_pe {
            source
                .replace("pe: pe1", "pe: pe0")
                .replace("pe: pe2", "pe: pe0")
        } else {
            source.to_owned()
        };
        let graph = TimetableFile::from_string(&source)
            .unwrap()
            .into_graph()
            .unwrap();
        let index = |name: &str| {
            graph
                .nodes()
                .iter()
                .position(|node| node.id() == name)
                .unwrap()
        };
        let (producer, first, second) = (index("produce"), index("consume1"), index("consume2"));
        let timetable = Timetable::new(engine.top(), graph, &platform).unwrap();
        let first_pe = if same_pe { "pe0" } else { "pe1" };
        let second_pe = if same_pe { "pe0" } else { "pe2" };
        assert!(
            !timetable
                .ready_task_indices(first_pe)
                .unwrap()
                .1
                .contains(&first)
        );
        assert!(
            !timetable
                .ready_task_indices(second_pe)
                .unwrap()
                .1
                .contains(&second)
        );
        timetable.set_task_active(producer).unwrap();
        assert!(
            !timetable
                .ready_task_indices(first_pe)
                .unwrap()
                .1
                .contains(&first)
        );
        timetable.set_task_completed(producer).unwrap();
        assert!(
            timetable
                .ready_task_indices(first_pe)
                .unwrap()
                .1
                .contains(&first)
        );
        assert!(
            timetable
                .ready_task_indices(second_pe)
                .unwrap()
                .1
                .contains(&second)
        );
    }
}

#[test]
fn shared_sram_admits_two_fused_tiles_only_when_both_arenas_fit() {
    let source = include_str!("../examples/fused_region.yaml");
    // Three 16-byte inputs, one 16-byte output, and a 16-byte local
    // intermediate make each tile's aligned arena exactly 80 bytes.
    let (overlap_ticks, overlap_events) = run_vector_timetable_with_sram(source, 160);
    let (serialized_ticks, serialized_events) = run_vector_timetable_with_sram(source, 80);
    let second_tile_read_position = |events: &[String]| {
        events
            .iter()
            .position(|event| event.contains("MemoryAccess: ReadRequest: 16@1010"))
            .expect("second tile A read")
    };
    let first_tile_store_position = |events: &[String]| {
        events
            .iter()
            .position(|event| event.contains("MemoryAccess: WriteNonPostedRequest: 16@5000"))
            .expect("first tile Y store")
    };
    assert!(
        second_tile_read_position(&overlap_events) < first_tile_store_position(&overlap_events)
    );
    assert!(
        second_tile_read_position(&serialized_events)
            > first_tile_store_position(&serialized_events)
    );
    assert!(overlap_ticks < serialized_ticks);
}

fn run_vector_timetable_with_sram(source: &str, sram_bytes: usize) -> (u64, Vec<String>) {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_yaml = include_str!("../examples/compute_region_platform.yaml").replace(
        "sram_bytes: 256",
        &format!("sram_bytes: {sram_bytes}\n      lsu_access_bytes: 16"),
    );
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml).unwrap());
    let graph = TimetableFile::from_string(source)
        .unwrap()
        .into_graph()
        .unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let ops = platform.pe("pe0").unwrap().machine_ops();
    assert_eq!((ops.by_operation["add"], ops.by_operation["mul"]), (8, 8));
    (clock.tick_now().tick(), tracker.events())
}

#[test]
fn a_tile_one_byte_larger_than_sram_fails_before_loading() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_yaml = include_str!("../examples/compute_region_platform.yaml")
        .replace("sram_bytes: 256", "sram_bytes: 79");
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml).unwrap());
    let graph = TimetableFile::from_string(include_str!("../examples/fused_region.yaml"))
        .unwrap()
        .into_graph()
        .unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    let error = engine.run().unwrap_err();
    assert!(error.0.contains("requires 80 SRAM bytes"), "{error:?}");
    assert!(
        !tracker
            .events()
            .iter()
            .any(|event| event.contains("MemoryAccess:"))
    );
}

#[test]
fn missing_typed_engine_fails_before_loading() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_yaml = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: mul", "class: compare");
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml).unwrap());
    let graph = TimetableFile::from_string(include_str!("../examples/fused_region.yaml"))
        .unwrap()
        .into_graph()
        .unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    let error = engine.run().unwrap_err();
    assert!(error.0.contains("unsupported capability"), "{error:?}");
    assert!(
        !tracker
            .events()
            .iter()
            .any(|event| event.contains("MemoryAccess:"))
    );
}

#[test]
fn compute_completion_tick_overflow_is_an_error() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker;
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_yaml = include_str!("../examples/compute_region_platform.yaml")
        .replace("ops_per_tick: 1", "ops_per_tick: 5e-19");
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml).unwrap());
    let graph = TimetableFile::from_string(include_str!("../examples/fused_region.yaml"))
        .unwrap()
        .into_graph()
        .unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    let error = engine.run().unwrap_err();
    assert!(
        error.0.contains("compute completion tick overflows"),
        "{error:?}"
    );
}

#[test]
fn explicit_matrix_mac_requires_its_capability_even_with_scalar_add_and_mul() {
    let (_test_tracker, tracker) = gwr_track::test_init!(1000);
    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let platform = Rc::new(
        Platform::from_string(
            &engine,
            &clock,
            include_str!("../examples/compute_region_platform.yaml"),
        )
        .unwrap(),
    );
    let file = TimetableFile::from_string(r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [2, 3] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [3, 2] } }
  - id: gemm
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [m, n]
          expr: { matrix_mac: { over: [k], args: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] } }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [2, 2] } }
edges:
  - { from: A, to: gemm.0, kind: data }
  - { from: B, to: gemm.1, kind: data }
  - { from: gemm.0, to: Y, kind: data }
").unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    let error = engine.run().unwrap_err();
    assert!(error.to_string().contains("matrix_mac"), "{error}");
}

#[test]
fn matrix_shaped_sum_of_products_uses_scalar_operations() {
    let (_test_tracker, tracker) = gwr_track::test_init!(1000);
    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let platform = Rc::new(
        Platform::from_string(
            &engine,
            &clock,
            include_str!("../examples/compute_region_platform.yaml"),
        )
        .unwrap(),
    );
    let file = TimetableFile::from_string(r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [2, 3] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [3, 2] } }
  - id: gemm
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [m, n]
          expr: { reduce: { op: sum, over: [k], value: { mul: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] } } }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp32, shape: [2, 2] } }
edges:
  - { from: A, to: gemm.0, kind: data }
  - { from: B, to: gemm.1, kind: data }
  - { from: gemm.0, to: Y, kind: data }
").unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let counts = platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(counts.by_operation["mul"], 12);
    assert_eq!(counts.by_operation["add"], 8);
    assert!(!counts.by_operation.contains_key("matrix_mac"));
}

#[test]
fn tiled_matrix_reduction_keeps_partials_local_and_counts_macs() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(
        Platform::from_string(
            &engine,
            &clock,
            r"
memory_maps:
  - name: mm
    devices: [mem]
operation_classes:
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: 256
      compute:
        engines:
          matrix:
            capabilities: [{ class: matrix, input_dtypes: [[fp16, fp16]], ops_per_tick: 10 }]
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: { capacity_bytes: 1MiB }
connections:
  - connect: [pe.pe0, mem.mem]
",
        )
        .unwrap(),
    );
    let file = TimetableFile::from_string(r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp16, shape: [2, 5] } }
  - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp16, shape: [5, 3] } }
  - id: gemm
    kind: compute
    pe: pe0
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [m, n]
          expr: { matrix_mac: { over: [k], args: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] } }
    tiling: { sizes: { m: 2, n: 3, k: 2 } }
  - { id: Y, kind: tensor, config: { addr: 0x3000, dtype: fp16, shape: [2, 3] } }
edges:
  - { from: A, to: gemm.0, kind: data }
  - { from: B, to: gemm.1, kind: data }
  - { from: gemm.0, to: Y, kind: data }
").unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let ops = platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(ops.by_operation["matrix_mac"], 30);
    assert!(!ops.by_operation.contains_key("add"));
    assert_eq!(timetable.machine_operations_executed().unwrap(), 30);
    assert_eq!(clock.tick_now().tick(), 78);
    let requests = tracker
        .events()
        .into_iter()
        .filter(|event| event.contains("created object") && event.contains("MemoryAccess:"))
        .collect::<Vec<_>>();
    let bytes = |kind: &str| {
        requests
            .iter()
            .filter(|event| event.contains(kind))
            .map(|event| {
                event
                    .split("MemoryAccess: ")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .split('@')
                    .next()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
            })
            .sum::<usize>()
    };
    assert_eq!(bytes("ReadRequest"), 288);
    assert_eq!(bytes("WriteNonPostedRequest"), 32);
}

#[test]
#[expect(clippy::too_many_lines)]
fn tiled_window_max_counts_valid_elements_and_stores_index_result() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(
        Platform::from_string(
            &engine,
            &clock,
            r"
memory_maps:
  - name: mm
    devices: [mem]
operation_classes:
  compare: { operations: [compare, maximum], arity: 2 }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: 256
      compute:
        engines:
          compare:
            capabilities: [{ class: compare, input_dtypes: [[fp32, fp32]], ops_per_tick: 4 }]
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: { capacity_bytes: 1MiB }
connections:
  - connect: [pe.pe0, mem.mem]
",
        )
        .unwrap(),
    );
    let file = TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [5] } }
  - id: pool
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y, I]
      extents: { o: 3, k: 3 }
      assignments:
        - output: Y
          indices_output: I
          axes: [o]
          expr:
            reduce:
              op: max
              over: [k]
              value:
                read:
                  tensor: A
                  indices: [{ affine: { terms: { o: 2, k: 1 }, offset: -1 } }]
                  bounds: exclude
    tiling: { sizes: { o: 1, k: 1 } }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [3] } }
  - { id: I, kind: tensor, config: { addr: 0x3000, dtype: int64, shape: [3] } }
edges:
  - { from: A, to: pool.0, kind: data }
  - { from: pool.0, to: Y, kind: data }
  - { from: pool.1, to: I, kind: data }
",
    )
    .unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let ops = platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(ops.by_operation["compare"], 4);
    let requests = tracker
        .events()
        .into_iter()
        .filter(|event| event.contains("created object") && event.contains("MemoryAccess:"))
        .collect::<Vec<_>>();
    let bytes = |kind: &str| {
        requests
            .iter()
            .filter(|event| event.contains(kind))
            .map(|event| {
                event
                    .split("MemoryAccess: ")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .split('@')
                    .next()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
            })
            .sum::<usize>()
    };
    assert_eq!(bytes("ReadRequest"), 224);
    assert_eq!(bytes("WriteNonPostedRequest"), 192);
}

#[test]
fn chunked_index_only_reduction_reserves_winning_value_sram() {
    let source = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1, 4] } }
  - id: argmax
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [I]
      assignments:
        - indices_output: I
          axes: [i]
          expr: { reduce: { op: max, over: [k], value: { read: { tensor: A, indices: [i, k] } } } }
    tiling: { sizes: { k: 2 } }
  - { id: I, kind: tensor, config: { addr: 0x2000, dtype: int64, shape: [1] } }
edges:
  - { from: A, to: argmax.0, kind: data }
  - { from: argmax.0, to: I, kind: data }
";
    let run = |sram_bytes: usize| {
        let (_test_tracker, tracker) = gwr_track::test_init!(1000);
        let mut engine = Engine::new(&tracker);
        let clock = engine.default_clock();
        let platform_yaml = format!(
            r"
memory_maps:
  - name: mm
    devices: [mem]
operation_classes:
  compare: {{ operations: [compare, maximum], arity: 2 }}
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: {sram_bytes}
      compute:
        engines:
          vector:
            capabilities: [{{ class: compare, ops_per_tick: 4 }}]
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: {{ capacity_bytes: 1MiB }}
connections:
  - connect: [pe.pe0, mem.mem]
"
        );
        let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml).unwrap());
        let file = TimetableFile::from_string(source).unwrap();
        let timetable =
            Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
        let dispatcher: Rc<dyn Dispatch> = timetable.clone();
        platform.attach_dispatcher(&dispatcher);
        engine.run()
    };
    let error = run(16).unwrap_err();
    assert!(error.0.contains("tile requires 24 SRAM bytes"), "{error:?}");
    run(24).unwrap();
}

#[test]
fn dilated_window_transfers_only_accessed_elements() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: add", "class: compare");
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_source).unwrap());
    let file = TimetableFile::from_string(
        r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [5] } }
  - id: pool
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      extents: { o: 1, k: 3 }
      assignments:
        - output: Y
          axes: [o]
          expr:
            reduce:
              op: max
              over: [k]
              value:
                read:
                  tensor: A
                  indices: [{ affine: { terms: { o: 1, k: 2 }, offset: 0 } }]
                  bounds: exclude
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [1] } }
edges:
  - { from: A, to: pool.0, kind: data }
  - { from: pool.0, to: Y, kind: data }
",
    )
    .unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let read_bytes = tracker
        .events()
        .iter()
        .filter(|event| event.contains("MemoryAccess: ReadRequest:"))
        .map(|event| {
            event
                .split("MemoryAccess: ReadRequest: ")
                .nth(1)
                .unwrap()
                .split('@')
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap()
        })
        .sum::<usize>();
    assert_eq!(read_bytes, 96);
    let gwr_models::processing_element::task::Task::ProgramTask { config } =
        timetable.task_by_id(1).unwrap()
    else {
        panic!("expected program task")
    };
    assert_eq!(config.planned_traffic_bytes().unwrap(), (12, 4));
    assert_eq!(
        platform.pe("pe0").unwrap().machine_ops().by_operation["compare"],
        2
    );
    assert_eq!(platform.pe("pe0").unwrap().peak_sram_bytes(), 24);
}

#[test]
fn max_reduction_counts_broadcast_singleton_terms() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_source = include_str!("../examples/compute_region_platform.yaml")
        .replace("class: add", "class: compare");
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_source).unwrap());
    let file = TimetableFile::from_string(r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1] } }
  - id: pool
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      extents: { k: 3 }
      assignments:
        - output: Y
          axes: []
          expr: { reduce: { op: max, over: [k], value: { read: { tensor: A, indices: [k], bounds: exclude } } } }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [] } }
edges:
  - { from: A, to: pool.0, kind: data }
  - { from: pool.0, to: Y, kind: data }
").unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    assert_eq!(
        platform.pe("pe0").unwrap().machine_ops().by_operation["compare"],
        2
    );
    let read_request_count = tracker
        .events()
        .iter()
        .filter(|event| event.contains("MemoryAccess: ReadRequest: 32@1000"))
        .count();
    assert_eq!(read_request_count, 1);
}

#[test]
#[expect(clippy::too_many_lines)]
fn tiled_fused_matrix_epilogue_runs_with_working_set_larger_than_sram() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform_source = r"
memory_maps:
  - name: mm
    devices: [mem]
operation_classes:
  matrix: { operations: [matrix_mac], arity: 2, flops_per_op: 2 }
  add: { operations: [add], arity: 2 }
  compare: { operations: [compare, maximum], arity: 2 }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: 16384
      compute:
        engines:
          matrix:
            capabilities: [{ class: matrix, input_dtypes: [[fp16, fp16]], ops_per_tick: 1024 }]
          vector:
            capabilities:
              - { class: add, input_dtypes: [[fp32, fp32]], ops_per_tick: 1024 }
              - { class: compare, input_dtypes: [[fp32, fp32]], ops_per_tick: 1024 }
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: { capacity_bytes: 1MiB }
connections:
  - connect: [pe.pe0, mem.mem]
";
    let platform = Rc::new(Platform::from_string(&engine, &clock, platform_source).unwrap());
    let source = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp16, shape: [64, 256] } }
  - { id: B, kind: tensor, config: { addr: 0x9000, dtype: fp16, shape: [256, 64] } }
  - { id: bias, kind: tensor, config: { addr: 0x11000, dtype: fp32, shape: [64] } }
  - { id: floor, kind: tensor, config: { addr: 0x12000, dtype: fp32, shape: [1] } }
  - id: fused
    kind: compute
    pe: pe0
    program:
      inputs: [A, B, bias, floor]
      outputs: [Y]
      temporaries: { product: { dtype: fp32 }, biased: { dtype: fp32 } }
      assignments:
        - output: product
          axes: [m, n]
          expr: { matrix_mac: { over: [k], args: [{ read: { tensor: A, indices: [m, k] } }, { read: { tensor: B, indices: [k, n] } }] } }
        - output: biased
          axes: [m, n]
          expr: { add: [{ read: { tensor: product, indices: [m, n] } }, { read: { tensor: bias, indices: [n] } }] }
        - output: Y
          axes: [m, n]
          expr: { maximum: [{ read: { tensor: biased, indices: [m, n] } }, { read: { tensor: floor, indices: [0] } }] }
    tiling: { sizes: { m: 16, n: 16, k: 32 } }
  - { id: Y, kind: tensor, config: { addr: 0x13000, dtype: fp32, shape: [64, 64] } }
edges:
  - { from: A, to: fused.0, kind: data }
  - { from: B, to: fused.1, kind: data }
  - { from: bias, to: fused.2, kind: data }
  - { from: floor, to: fused.3, kind: data }
  - { from: fused.0, to: Y, kind: data }
";
    let file = TimetableFile::from_string(source).unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let ops = platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(
        (
            ops.by_operation["matrix_mac"],
            ops.by_operation["add"],
            ops.by_operation["maximum"]
        ),
        (1_048_576, 4096, 4096)
    );
    let requests = tracker
        .events()
        .into_iter()
        .filter(|event| event.contains("created object") && event.contains("MemoryAccess:"))
        .collect::<Vec<_>>();
    let bytes = |kind: &str| {
        requests
            .iter()
            .filter(|event| event.contains(kind))
            .map(|event| {
                event
                    .split("MemoryAccess: ")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .split('@')
                    .next()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
            })
            .sum::<usize>()
    };
    assert_eq!(bytes("ReadRequest"), 263_680);
    assert_eq!(bytes("WriteNonPostedRequest"), 16_384);

    // One output tile has 1024-byte A and B slices, a 64-byte bias slice,
    // an 8-byte aligned floor, a 1024-byte output, and two 1024-byte locals:
    // 5192 bytes. The full 64x64x256 logical working set is much larger.
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(
        Platform::from_string(
            &engine,
            &clock,
            &platform_source.replace("sram_bytes: 16384", "sram_bytes: 5191"),
        )
        .unwrap(),
    );
    let graph = TimetableFile::from_string(source)
        .unwrap()
        .into_graph()
        .unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    let error = engine.run().unwrap_err();
    assert!(error.0.contains("requires 5192 SRAM bytes"), "{error:?}");
    assert!(
        !tracker
            .events()
            .iter()
            .any(|event| event.contains("MemoryAccess:"))
    );
}
