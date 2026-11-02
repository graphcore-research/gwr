// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::test_init;

#[test]
fn declared_unary_operation_reports_operation_counts_and_memory_traffic() {
    let platform_yaml = r"
operation_classes:
  trig: { operations: [sin, cos, tan], arity: 1 }
memory_maps:
  - { name: mm, devices: [mem] }
processing_elements:
  - name: pe0
    memory_map: mm
    config:
      sram_bytes: 1KiB
      compute:
        execution_units:
          trig_engine:
            capabilities:
              - { class: trig, input_dtypes: [[fp32]], ops_per_tick: 2 }
memories:
  - { name: mem, kind: hbm, base_address: 0, config: { capacity_bytes: 1MiB, delay_ticks: 2 } }
connections:
  - { connect: [pe.pe0, mem.mem] }
";
    let timetable_yaml = r"
nodes:
  - { id: A, kind: tensor, config: { addr: 0x1000, dtype: fp32, shape: [4] } }
  - id: sin_node
    kind: compute
    pe: pe0
    program:
      inputs: [A]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            sin:
              - read: { tensor: A, indices: [i] }
  - { id: Y, kind: tensor, config: { addr: 0x2000, dtype: fp32, shape: [4] } }
edges:
  - { from: A, to: sin_node.0, kind: data }
  - { from: sin_node.0, to: Y, kind: data }
";
    let graph = TimetableFile::from_string(timetable_yaml)
        .unwrap()
        .into_graph()
        .unwrap();
    let (_test_tracker, tracker) = test_init!(1000);
    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, platform_yaml).unwrap());
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let counts = platform.pe("pe0").unwrap().machine_ops();
    assert_eq!(counts.by_operation["sin"], 4);
    assert_eq!(counts.flop_equivalents, 4);
    assert_eq!(platform.pe("pe0").unwrap().memory_traffic_bytes(), (16, 16));
}
