// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn initial_tensor_dependencies_complete_in_any_file_order() {
    let timetable = timetable(
        r"
nodes:
 - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [1] } }
 - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1] } }
 - { id: consumer, kind: compute, pe: pe0, program: { inputs: [], outputs: [] } }
edges:
 - { from: A, to: B, kind: dependency }
 - { from: B, to: consumer, kind: dependency }
",
    );
    assert_eq!(timetable.num_graph_nodes_completed(), 2);
    assert_eq!(
        timetable.ready_task_indices("pe0").unwrap(),
        (false, vec![2])
    );
}

#[test]
fn completed_task_propagates_through_tensor_dependency_chains() {
    let timetable = timetable(
        r"
nodes:
 - { id: producer, kind: compute, pe: pe0, program: { inputs: [], outputs: [] } }
 - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1] } }
 - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [1] } }
 - { id: consumer, kind: compute, pe: pe0, program: { inputs: [], outputs: [] } }
edges:
 - { from: producer, to: A, kind: dependency }
 - { from: A, to: B, kind: dependency }
 - { from: B, to: consumer, kind: dependency }
",
    );
    timetable.enable_completed_workload_tracking().unwrap();
    assert_eq!(
        timetable.ready_task_indices("pe0").unwrap(),
        (false, vec![0])
    );
    timetable.set_task_completed(0).unwrap();
    assert_eq!(timetable.num_graph_nodes_completed(), 3);
    assert_eq!(timetable.completed_workload().unwrap().nodes, 3);
    assert_eq!(
        timetable.ready_task_indices("pe0").unwrap(),
        (false, vec![3])
    );
    timetable.set_task_completed(3).unwrap();
    timetable.check_tasks_complete().unwrap();
}

#[test]
fn tensor_dependency_join_waits_for_every_producer() {
    let timetable = timetable(
        r"
nodes:
 - { id: left, kind: compute, pe: pe0, program: { inputs: [], outputs: [] } }
 - { id: right, kind: compute, pe: pe0, program: { inputs: [], outputs: [] } }
 - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [1] } }
 - { id: B, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [1] } }
 - { id: consumer, kind: compute, pe: pe0, program: { inputs: [], outputs: [] } }
edges:
 - { from: left, to: A, kind: dependency }
 - { from: A, to: B, kind: dependency }
 - { from: right, to: B, kind: dependency }
 - { from: B, to: consumer, kind: dependency }
",
    );
    timetable.set_task_completed(0).unwrap();
    assert_eq!(
        timetable.ready_task_indices("pe0").unwrap(),
        (false, vec![1])
    );
    timetable.set_task_completed(1).unwrap();
    assert_eq!(
        timetable.ready_task_indices("pe0").unwrap(),
        (false, vec![4])
    );
}

fn timetable(source: &str) -> Timetable {
    let tracking: gwr_track::Tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
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
    Timetable::new(
        engine.top(),
        TimetableFile::from_string(source)
            .unwrap()
            .into_graph()
            .unwrap(),
        &platform,
    )
    .unwrap()
}
