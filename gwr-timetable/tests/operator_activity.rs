// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

const PLATFORM_YAML: &str = "
memory_maps:
  - name: pe_memory_map
    devices:
      - name: mem0

processing_elements:
  - name: pe0
    memory_map: pe_memory_map
    config:
      lsu_access_bytes: 32
      sram_bytes: 64KiB

caches:
  - name: l1_0
    config:
      bw_bytes_per_cycle: 32
      line_size_bytes: 32
      delay_ticks: 4

memories:
  - name: mem0
    kind: ddr
    base_address: 0x1_0000_0000
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
    op: add
    pe: pe0
    input_views:
      -
      -
    output_views:
      -

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

#[test]
fn compute_task_emits_operator_activity_on_pe() {
    let (test_tracker, tracker) = gwr_track::test_init!(1000);
    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, PLATFORM_YAML).unwrap());
    let timetable_file = TimetableFile::from_string(TIMETABLE_YAML).unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), timetable_file, &platform).unwrap());
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
        events
            .iter()
            .any(|event| event.contains("created lane") && event.contains("pe0::lane::compute::0")),
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
            .any(|event| event.contains("created group") && event.contains("pe0::add operation")),
        "missing PE operation group create event in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("activity begin add tensor 0 read")),
        "missing operator tensor 0 read activity begin event in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("activity begin add tensor 1 read")),
        "missing operator tensor 1 read activity begin event in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("activity begin add compute")),
        "missing operator compute activity begin event in {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("activity begin add tensor 0 write")),
        "missing operator tensor 0 write activity begin event in {events:#?}"
    );
    let read_group =
        activity_group(&events, "activity begin add tensor 0 read").expect("missing read group");
    let compute_group =
        activity_group(&events, "activity begin add compute").expect("missing compute group");
    let write_group =
        activity_group(&events, "activity begin add tensor 0 write").expect("missing write group");
    assert_eq!(read_group, compute_group);
    assert_eq!(compute_group, write_group);
    assert!(
        events.iter().any(|event| event.contains("activity end")),
        "missing operator activity end event in {events:#?}"
    );
}

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
