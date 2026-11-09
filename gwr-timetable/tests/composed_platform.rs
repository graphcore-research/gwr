// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use std::rc::Rc;
#[test]
fn explicit_timetable_transfer_uses_parent_bound_dma_map_between_packages() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../gwr-platform/examples/composition");
    let mut engine = gwr_engine::engine::Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let platform = Rc::new(
        gwr_platform::Platform::from_file(&engine, &clock, &dir.join("system.yaml")).unwrap(),
    );
    let file = gwr_timetable::timetable_file::TimetableFile::from_file(&dir.join("transfer.yaml"))
        .unwrap();
    let timetable = Rc::new(
        gwr_timetable::Timetable::new(engine.top(), file.into_graph().unwrap(), &platform).unwrap(),
    );
    let dispatcher: Rc<dyn gwr_models::processing_element::dispatch::Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    assert_eq!(
        platform
            .memory("rack[0]/board[0]/chip[1]/hbm")
            .unwrap()
            .bytes_read(),
        64
    );
    assert_eq!(
        platform
            .memory("rack[0]/board[0]/chip[0]/hbm")
            .unwrap()
            .bytes_written(),
        64
    );
    assert_eq!(
        platform
            .memory("rack[1]/board[0]/chip[0]/hbm")
            .unwrap()
            .bytes_read(),
        0
    );
}
