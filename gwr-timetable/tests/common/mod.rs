// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::time::clock::Clock;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::Tracker;
use gwr_track::log::Level;
use gwr_track::test_helpers::TestTracker;

pub struct Simulation {
    pub platform: Rc<Platform>,
    pub tracker: Rc<TestTracker>,
    pub clock: Clock,
    pub timetable: Rc<Timetable>,
}

pub fn run_timetable(platform_source: &str, file: TimetableFile) -> Simulation {
    let (mut engine, clock, platform, tracker) = fixture(platform_source);
    let timetable =
        Rc::new(Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap());
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    let simulation = Simulation {
        platform,
        tracker,
        clock,
        timetable,
    };
    simulation.timetable.check_tasks_complete().unwrap();
    let planned = simulation.timetable.workload_totals().unwrap();
    assert_eq!(
        simulation
            .platform
            .pe("pe0")
            .unwrap()
            .memory_traffic_bytes(),
        (planned.read_bytes, planned.written_bytes),
        "planned boundary traffic must match completed execution at tick {}",
        simulation.clock.tick_now().tick(),
    );
    simulation
}

pub fn assert_rejected_before_loading(
    platform_source: &str,
    file: TimetableFile,
    expected: &[&str],
) {
    let (engine, clock, platform, tracker) = fixture(platform_source);
    let error = Timetable::new(engine.top(), file.into_graph().unwrap(), &platform)
        .err()
        .unwrap();
    for expected in expected {
        assert!(error.0.contains(expected), "{error:?}");
    }
    assert_eq!(clock.tick_now().tick(), 0);
    let memory = platform.memory("mem").unwrap();
    assert_eq!((memory.bytes_read(), memory.bytes_written()), (0, 0));
    let pe = platform.pe("pe0").unwrap();
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert_eq!(pe.machine_ops().checked_operation_total().unwrap(), 0);
    assert!(
        !tracker
            .events()
            .iter()
            .any(|event| event.contains("MemoryAccess:"))
    );
}

fn fixture(platform_source: &str) -> (Engine, Clock, Rc<Platform>, Rc<TestTracker>) {
    let tracker = Rc::new(TestTracker::new(1000, Level::Trace));
    let tracking: Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, platform_source).unwrap());
    (engine, clock, platform, tracker)
}
