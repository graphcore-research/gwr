// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

// ANCHOR: example
use std::path::Path;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_file(
        &engine,
        &clock,
        &examples.join("compute_region_platform.yaml"),
    )?);
    let graph = TimetableFile::from_file(&examples.join("add_mul.yaml"))?.into_graph()?;
    // This prepares every compute node against its assigned PE before
    // execution.
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform)?);
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run()?;
    timetable.check_tasks_complete()?;
    println!("Completed at tick {}", clock.tick_now().tick());
    Ok(())
}
// ANCHOR_END: example
