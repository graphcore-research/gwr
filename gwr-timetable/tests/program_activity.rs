// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn compute_region_emits_tile_activity_on_pe() {
    let (test_tracker, tracker) = gwr_track::test_init!(1000);
    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, PLATFORM_YAML).unwrap());
    let timetable_file = TimetableFile::from_string(TIMETABLE_YAML).unwrap();
    let graph = timetable_file.into_graph().unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);

    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();

    let events = test_tracker.events();
    assert!(
        events
            .iter()
            .any(|event| event.contains("created lane") && event.contains("pe0::lane::lsu_read::0")),
        "missing PE LSU read lane create event in {events:#?}"
    );
    assert!(
        events.iter().any(|event| event.contains("created lane")
            && event.contains("pe0::lane::compute::vector::0")),
        "missing PE compute lane create event in {events:#?}"
    );
    assert!(
        events.iter().any(
            |event| event.contains("created lane") && event.contains("pe0::lane::lsu_write::0")
        ),
        "missing PE LSU write lane create event in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("created group") && event.contains("pe0::add tile")),
        "missing PE tile group create event in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("activity begin add A read")),
        "missing A read activity begin event in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("activity begin add B read")),
        "missing B read activity begin event in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("activity begin add Y compute")),
        "missing assignment compute activity begin event in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("activity begin add Y write")),
        "missing Y write activity begin event in {events:#?}"
    );
    let read_group =
        activity_group(&events, "activity begin add A read").expect("missing read group");
    let compute_group =
        activity_group(&events, "activity begin add Y compute").expect("missing compute group");
    let write_group =
        activity_group(&events, "activity begin add Y write").expect("missing write group");
    assert_eq!(read_group, compute_group);
    assert_eq!(compute_group, write_group);
    assert!(
        events.iter().any(|event| event.contains("activity end")),
        "missing compute activity end event in {events:#?}"
    );
}

const PLATFORM_YAML: &str = "
memory_maps:
  - name: pe_memory_map
    devices:
      - mem0

operation_classes:
  add: { operations: [add], arity: 2 }

processing_elements:
  - name: pe0
    memory_map: pe_memory_map
    config:
      lsu_access_bytes: 32
      sram_bytes: 64KiB
      compute:
        engines:
          vector:
            capabilities: [{ class: add, ops_per_tick: 256 }]

caches:
  - name: l1_0
    config:
      bw_bytes_per_tick: 32
      line_size_bytes: 32
      delay_ticks: 4

memories:
  - name: mem0
    kind: ddr
    base_address: 0x1_0000_0000
    config:
      capacity_bytes: 1GiB
      delay_ticks: 40

connections:
  - connect:
      - pe.pe0
      - cache.l1_0.dev
  - connect:
      - cache.l1_0.mem
      - mem.mem0
";

const TIMETABLE_YAML: &str = "
nodes:
  - id: input_a
    kind: tensor
    config:
      addr: 0x1_0000_0000
      dtype: fp32
      shape: [4]

  - id: input_b
    kind: tensor
    config:
      addr: 0x1_0000_0400
      dtype: fp32
      shape: [4]

  - id: add
    kind: compute
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr: { add: [{ read: { tensor: A, indices: [i] } }, { read: { tensor: B, indices: [i] } }] }
    pe: pe0

  - id: output
    kind: tensor
    config:
      addr: 0x1_0000_0800
      dtype: fp32
      shape: [4]

edges:
  - from: input_a
    to: add.0
    kind: data

  - from: input_b
    to: add.1
    kind: data

  - from: add
    to: output
    kind: data
";

fn activity_group(events: &[String], activity_name: &str) -> Option<String> {
    let mut lane_groups = HashMap::new();
    for event in events {
        let Some((lane, rest)) = event.split_once(": ") else {
            continue;
        };
        if let Some(group) = rest.strip_prefix("added to group ") {
            lane_groups.insert(lane.to_string(), group.to_string());
        } else if let Some(group) = rest.strip_prefix("removed from group ") {
            if lane_groups
                .get(lane)
                .is_some_and(|active_group| active_group == group)
            {
                lane_groups.remove(lane);
            }
        } else if rest.contains(activity_name) {
            return lane_groups.get(lane).cloned();
        }
    }
    None
}
