// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Structured, trace-free measurement driver for calibration/run.py.
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use gwr_engine::engine::Engine;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_platform::Platform;
use gwr_platform::catalog::{CatalogTarget, load_yaml_with_includes};
use gwr_platform::types::PlatformConfig;
use gwr_timetable::Timetable;
use gwr_timetable::analysis::roofline::RooflineAnalyzer;
use gwr_timetable::timetable_file::TimetableFile;
use gwr_track::tracker::dev_null_tracker;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 4 {
        return Err(
            "usage: calibrate resolve|simulated|aggregate-roofline PLATFORM TIMETABLE [PERIOD_PS]"
                .into(),
        );
    }
    let total = Instant::now();
    let platform_yaml = load_yaml_with_includes(Path::new(&args[2]), CatalogTarget::Platform)?;
    let timetable_yaml = load_yaml_with_includes(Path::new(&args[3]), CatalogTarget::Timetable)?;
    if args[1] == "resolve" {
        println!(
            "{}",
            json!({"platform":platform_yaml,"timetable":timetable_yaml})
        );
        return Ok(());
    }
    let period_ps: u64 = args.get(4).map_or(Ok(1000), |v| v.parse())?;
    if period_ps == 0 {
        return Err("period must be positive".into());
    }
    let config: PlatformConfig = yaml_serde::from_str(&platform_yaml)?;
    let graph = TimetableFile::from_string(&timetable_yaml)?.into_graph()?;
    let mut engine = Engine::new(&dev_null_tracker());
    let clock = engine.clock_hz(1e12 / period_ps as f64);
    if args[1].starts_with("aggregate-roofline") {
        let setup = total.elapsed().as_secs_f64();
        let start = Instant::now();
        let report = match args[1].as_str() {
            "aggregate-roofline" => RooflineAnalyzer::analyze_graph(&config, &graph)?,
            "aggregate-roofline-packets" => {
                RooflineAnalyzer::analyze_graph_lsu_aligned(&config, &graph)?
            }
            _ => return Err("unsupported roofline engine".into()),
        };
        println!(
            "{}",
            json!({"simulated_time_ns":report.estimated_ticks * period_ps as f64 / 1000.0,
            "ticks":report.estimated_ticks,"setup_s":setup,"evaluation_s":start.elapsed().as_secs_f64(),
            "memory_read":report.read_bytes_by_memory.values().sum::<u128>(),
            "memory_written":report.written_bytes_by_memory.values().sum::<u128>(),
            "report":report,"period_ps":period_ps})
        );
        return Ok(());
    }
    if args[1] != "simulated" {
        return Err("unsupported engine".into());
    }
    engine.set_task_order_seed(0);
    let platform = Rc::new(Platform::from_string(&engine, &clock, &platform_yaml)?);
    for name in platform.pe_names() {
        platform.pe(&name)?.enable_compute_action_counters()?;
    }
    let tasks = Rc::new(Timetable::new(engine.top(), graph, &platform)?);
    platform.attach_dispatcher(&(tasks.clone() as Rc<dyn Dispatch>));
    let setup = total.elapsed().as_secs_f64();
    let start = Instant::now();
    engine.run()?;
    let evaluation = start.elapsed().as_secs_f64();
    tasks.check_tasks_complete()?;
    let memories: Vec<_> = config.memories.iter().flatten().map(|m| {
        let model = platform.memory_model(&m.name).unwrap();
        json!({"name":m.name,"read_bytes":model.bytes_read(),"written_bytes":model.bytes_written()})
    }).collect();
    let caches: Vec<_> = config.caches.iter().flatten().map(|c| {
        let model = platform.cache_model(&c.name).unwrap();
        json!({"name":c.name,"read_bytes":model.payload_bytes_read(),"written_bytes":model.payload_bytes_written(),
            "hits":model.num_hits(),"misses":model.num_misses()})
    }).collect();
    let mut names = platform.pe_names();
    names.sort();
    let compute: Vec<_> = names
        .iter()
        .map(|name| {
            let pe = platform.pe(name).unwrap();
            let issue = pe.lsu_issue_snapshot();
            let (read, written) = pe.memory_traffic_bytes();
            let block = pe.block_execution_snapshot().map(|s| json!({
                "load_chunks": s.completed_load_chunks, "compute_chunks": s.completed_compute_chunks,
                "store_tiles": s.completed_store_tiles, "input_wait_ticks": s.input_wait_ticks,
                "output_wait_ticks": s.output_wait_ticks, "compute_wait_ticks": s.compute_wait_ticks,
                "load_compute_overlap_ticks": s.load_compute_overlap_ticks,
                "store_compute_overlap_ticks": s.store_compute_overlap_ticks,
                "local_read_bytes": s.local_read_bytes, "local_write_bytes": s.local_write_bytes
            }));
            json!({"name":name,"actions":pe.compute_action_snapshot(),
                "logical_read_bytes":read,"logical_written_bytes":written,"block":block,
                "lsu":{"issued_read_packets":issue.issued_read_packets,"issued_write_packets":issue.issued_write_packets,
                    "arbitration_wait_ticks":issue.arbitration_wait_ticks,"credit_wait_ticks":issue.credit_wait_ticks,
                    "peak_outstanding_packets":issue.peak_outstanding_packets,"outstanding_packets":issue.outstanding_packets}})
        })
        .collect();
    println!(
        "{}",
        json!({"simulated_time_ns":clock.time_now_ns(),"ticks":clock.tick_now().tick(),
        "setup_s":setup,"evaluation_s":evaluation,"period_ps":period_ps,
        "memory_read":memories.iter().map(|v|v["read_bytes"].as_u64().unwrap()).sum::<u64>(),
        "memory_written":memories.iter().map(|v|v["written_bytes"].as_u64().unwrap()).sum::<u64>(),
        "logical_operations":compute.iter().flat_map(|p|p["actions"].as_array().unwrap()).map(|a|a["logical_operations"].as_u64().unwrap()).sum::<u64>(),
        "completed_nodes":tasks.num_graph_nodes_completed(),"memories":memories,"caches":caches,"compute":compute,
        "coverage":["Fabric physical action counters are not exposed; memory payloads are boundary observations."]})
    );
    Ok(())
}
