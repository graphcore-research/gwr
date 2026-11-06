// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::test_helpers::start_test;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::entity::Entity;

fn create_platform() -> (Rc<Entity>, Rc<Platform>) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let platform = Platform::from_string(
        &engine,
        &clock,
        "
memory_maps:
  - name: default
    devices:
      - hbm0

operation_classes:
  compare: { operations: [compare], arity: 2 }

processing_elements:
  - name: pe0
    memory_map: default
    config:
      compute:
        execution_units:
          vector:
            capabilities: [{ class: compare, ops_per_tick: 16 }]

memories:
  - name: hbm0
    kind: hbm
    base_address: 0
    config:
      capacity_bytes: 0x1000_0000
",
    )
    .unwrap();

    (engine.top().clone(), Rc::new(platform))
}

#[test]
fn compute_completion_updates_multiple_outputs() {
    let (top, platform) = create_platform();
    let timetable_file = TimetableFile::from_string(
        r"
nodes:
  - id: input
    kind: tensor
    config:
      addr: 0
      dtype: fp32
      shape: [1, 1, 4, 4]

  - id: pool
    kind: compute
    program:
      inputs: [A]
      outputs: [Y, I]
      extents: { b: 1, c: 1, oh: 3, ow: 3, kh: 2, kw: 2 }
      assignments:
        - output: Y
          indices_output: I
          axes: [b, c, oh, ow]
          expr:
            reduce:
              op: max
              over: [kh, kw]
              value:
                read:
                  tensor: A
                  indices:
                    - b
                    - c
                    - affine:
                        terms: { oh: 1, kh: 1 }
                        offset: 0
                    - affine:
                        terms: { ow: 1, kw: 1 }
                        offset: 0
                  bounds: exclude
    pe: pe0

  - id: pooled
    kind: tensor
    config:
      addr: 1024
      dtype: fp32
      shape: [1, 1, 3, 3]

  - id: indices
    kind: tensor
    config:
      addr: 2048
      dtype: int64
      shape: [1, 1, 3, 3]

edges:
  - from: input
    to: pool.A
    kind: data

  - from: pool.Y
    to: pooled
    kind: data

  - from: pool.I
    to: indices
    kind: data
",
    )
    .unwrap();

    let timetable = Timetable::new(&top, timetable_file.into_graph().unwrap(), &platform).unwrap();

    timetable.set_task_completed(1).unwrap();
    timetable.set_task_completed(1).unwrap();

    let (done, ready_node_indices) = timetable.ready_task_indices("pe0").unwrap();
    assert!(done);
    assert!(ready_node_indices.is_empty());

    timetable.check_tasks_complete().unwrap();
}
