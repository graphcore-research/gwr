// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn copy_larger_than_sram_moves_real_bytes_with_no_compute_staging() {
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
    let file = TimetableFile::from_string(&source(32, "[4096]")).unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    assert_eq!(timetable.workload_totals().unwrap().read_bytes, 16384);
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    let pe = platform.pe("pe0").unwrap();
    assert_eq!(pe.memory_traffic_bytes(), (16384, 16384));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert_eq!(pe.total_flops(), 0);
    assert!(timetable.render_mermaid().contains("transfer"));
}

#[test]
fn invalid_transfer_size_and_buffer_are_rejected_before_dispatch() {
    for yaml in [source(0, "[4096]"), source(32, "[1]")] {
        assert!(
            TimetableFile::from_string(&yaml)
                .unwrap()
                .into_graph()
                .is_err()
        );
    }
}

#[test]
fn named_dma_routes_responses_independently_and_shares_memory() {
    let tracker = Rc::new(gwr_track::test_helpers::TestTracker::new(
        1000,
        gwr_track::log::Level::Trace,
    ));
    let tracking: gwr_track::Tracker = tracker;
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let yaml = include_str!("../examples/compute_region_platform.yaml").replace(
        "connections:\n  - connect: [pe.pe0, mem.mem]",
        r#"
fabrics:
  - name: switch
    kind: functional
    columns: 3
    rows: 1
    config: { port_bits_per_tick: 256, ticks_per_hop: 1, ticks_overhead: 0 }
connections:
  - connect: [pe.pe0, "fabric.switch@(0,0)"]
  - connect: [pe.pe0.dma, "fabric.switch@(1,0)"]
  - connect: [mem.mem, "fabric.switch@(2,0)"]
"#,
    );
    let platform = Rc::new(Platform::from_string(&engine, &clock, &yaml).unwrap());
    let file = TimetableFile::from_string(&source(128, "[4096]")).unwrap();
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    assert_eq!(
        platform.pe("pe0").unwrap().memory_traffic_bytes(),
        (16384, 16384)
    );
}

fn source(buffer: usize, shape: &str) -> String {
    format!(
        r"
nodes:
  - {{ id: A, kind: tensor, config: {{ addr: 0x1000, dtype: fp32, shape: [4096] }} }}
  - {{ id: copy, kind: transfer, pe: pe0, buffer_bytes: {buffer} }}
  - {{ id: Y, kind: tensor, config: {{ addr: 0x20000, dtype: fp32, shape: {shape} }} }}
edges:
  - {{ from: A, to: copy, kind: data }}
  - {{ from: copy, to: Y, kind: data }}
"
    )
}
