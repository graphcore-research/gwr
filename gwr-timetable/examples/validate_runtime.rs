// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Measurement-only driver: no tracing/progress, separate setup/evaluation.
use std::rc::Rc;
use std::time::Instant;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::Timetable;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::tracker::dev_null_tracker;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--convert") {
        let mut value: yaml_serde::Value =
            yaml_serde::from_str(&std::fs::read_to_string(&args[2])?)?;
        for section in ["processing_elements", "caches", "memories", "fabrics"] {
            if let Some(items) = value[section].as_sequence_mut() {
                for item in items {
                    if args[3] == "detailed" {
                        item.as_mapping_mut()
                            .unwrap()
                            .remove(yaml_serde::Value::from("model"));
                    } else {
                        item["model"] = yaml_serde::Value::from("roofline");
                    }
                }
            }
        }
        print!("{}", yaml_serde::to_string(&value)?);
        return Ok(());
    }
    let total = Instant::now();
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.default_clock();
    let yaml = std::fs::read_to_string(&args[2])?;
    let cfg: PlatformConfig = yaml_serde::from_str(&yaml)?;
    let platform = Rc::new(Platform::from_string(&engine, &clock, &yaml)?);
    let file = TimetableFile::from_file(std::path::Path::new(&args[1]))?;
    let tasks = Rc::new(Timetable::new(engine.top(), file.into_graph()?, &platform)?);
    platform.attach_dispatcher(&(tasks.clone() as Rc<dyn Dispatch>));
    let setup = total.elapsed().as_secs_f64();
    let start = Instant::now();
    engine.run()?;
    let evaluation = start.elapsed().as_secs_f64();
    tasks.check_tasks_complete()?;
    let elapsed = total.elapsed().as_secs_f64();
    let memories: Vec<_> = cfg
        .memories
        .iter()
        .flatten()
        .map(|m| {
            let model = platform.memory_model(&m.name).unwrap();
            (model.bytes_read(), model.bytes_written())
        })
        .collect();
    let caches: Vec<_> = cfg
        .caches
        .iter()
        .flatten()
        .map(|c| {
            let model = platform.cache_model(&c.name).unwrap();
            (
                model.payload_bytes_read(),
                model.payload_bytes_written(),
                model.num_hits(),
                model.num_misses(),
            )
        })
        .collect();
    println!(
        "{{\"setup_s\":{setup},\"evaluation_s\":{evaluation},\"total_s\":{elapsed},\"ticks\":{},\"memory_read\":{},\"memory_written\":{},\"cache_read\":{},\"cache_written\":{},\"hits\":{},\"misses\":{},\"completed_nodes\":{}}}",
        clock.tick_now().tick(),
        memories.iter().map(|m| m.0).sum::<usize>(),
        memories.iter().map(|m| m.1).sum::<usize>(),
        caches.iter().map(|c| c.0).sum::<usize>(),
        caches.iter().map(|c| c.1).sum::<usize>(),
        caches.iter().map(|c| c.2).sum::<usize>(),
        caches.iter().map(|c| c.3).sum::<usize>(),
        tasks.num_graph_nodes_completed()
    );
    Ok(())
}
