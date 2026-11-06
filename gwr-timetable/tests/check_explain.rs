// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::process::{Command, Output};

use gwr_timetable::timetable_file::TimetableFile;
use tempfile::tempdir;

const PLATFORM: &str = include_str!("../examples/compute_region_platform.yaml");
const TIMETABLE: &str = include_str!("../examples/add_mul.yaml");

fn cli(platform: &str, timetable: &str, flags: &[&str]) -> Output {
    let dir = tempdir().unwrap();
    let platform_path = dir.path().join("platform.yaml");
    let timetable_path = dir.path().join("timetable.yaml");
    fs::write(&platform_path, platform).unwrap();
    fs::write(&timetable_path, timetable).unwrap();
    Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .arg("--platform")
        .arg(platform_path)
        .arg("--timetable")
        .arg(timetable_path)
        .args(flags)
        .output()
        .unwrap()
}

fn success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn check_accepts_normal_preparation_without_running() {
    let normal = success(&cli(
        PLATFORM,
        TIMETABLE,
        &["--stdout", "--stdout-level", "trace"],
    ));
    assert!(normal.contains("Ran simulation."));
    assert!(normal.contains("MemoryAccess:"));
    assert!(normal.contains("activity begin"));
    for flag in ["--check", "--explain"] {
        let checked = success(&cli(
            PLATFORM,
            TIMETABLE,
            &[
                flag,
                "--progress",
                "--dump-stats",
                "--stdout",
                "--stdout-level",
                "trace",
            ],
        ));
        assert!(checked.contains("Preparation successful; simulation was not run."));
        assert!(!checked.contains("Ran simulation."));
        assert!(!checked.contains("Memory totals:"));
        assert!(!checked.contains("machine operations completed"));
        assert!(!checked.contains("MemoryAccess:"));
        assert!(!checked.contains("activity begin"));
    }
}

#[test]
fn explain_is_deterministic_and_reports_retained_facts() {
    let first = success(&cli(PLATFORM, TIMETABLE, &["--explain"]));
    for _ in 0..3 {
        assert_eq!(first, success(&cli(PLATFORM, TIMETABLE, &["--explain"])));
    }
    for fact in [
        "Node add on PE pe0",
        "input A <- A",
        "output T -> T",
        "shape=[8]",
        "extents: i=8",
        "declared tile sizes: i=4",
        "effective tile sizes: i=4",
        "peak SRAM: 48 bytes",
        "logical boundary traffic: read=64 bytes, write=32 bytes",
        "LSU access: 32 bytes",
        "SRAM capacity: 256 bytes",
        "physical traffic is not observed",
    ] {
        assert!(first.contains(fact), "missing {fact}:\n{first}");
    }
    assert!(!first.contains("Ran simulation."));
    assert!(!first.contains("PreparedProgram {"));
}

#[test]
fn check_and_explain_preserve_constructor_errors() {
    for (platform, timetable) in [
        (
            PLATFORM.to_owned(),
            TIMETABLE.replace("pe: pe0", "pe: missing"),
        ),
        (
            PLATFORM.to_owned(),
            TIMETABLE.replace("add.A", "add.missing"),
        ),
        (
            PLATFORM.replace("sram_bytes: 256", "sram_bytes: 40"),
            TIMETABLE.to_owned(),
        ),
        (
            PLATFORM.to_owned(),
            TIMETABLE.replace("add:", "unknown_op:"),
        ),
        (
            PLATFORM.replace("capacity_bytes: 1MiB", "capacity_bytes: 4KiB"),
            TIMETABLE.to_owned(),
        ),
        (
            PLATFORM.replace(
                "sram_bytes: 256",
                "sram_bytes: 256\n      lsu_access_bytes: 0",
            ),
            TIMETABLE.to_owned(),
        ),
    ] {
        let normal = cli(&platform, &timetable, &[]);
        assert!(
            !normal.status.success(),
            "invalid fixture unexpectedly ran successfully"
        );
        for flag in ["--check", "--explain"] {
            let checked = cli(&platform, &timetable, &[flag]);
            assert_eq!(normal.status.success(), checked.status.success());
            assert_eq!(normal.stderr, checked.stderr);
        }
    }
}

#[test]
fn explanation_does_not_execute_or_change_prepared_state() {
    use std::rc::Rc;

    use gwr_engine::engine::Engine;
    use gwr_models::processing_element::dispatch::Dispatch;
    use gwr_platform::Platform;
    use gwr_timetable::Timetable;
    use gwr_track::Tracker;
    use gwr_track::log::Level;
    use gwr_track::test_helpers::TestTracker;

    let tracker = Rc::new(TestTracker::new(1000, Level::Trace));
    let tracking: Tracker = tracker.clone();
    let mut engine = Engine::new(&tracking);
    let clock = engine.default_clock();
    let platform = Rc::new(Platform::from_string(&engine, &clock, PLATFORM).unwrap());
    let graph = TimetableFile::from_string(TIMETABLE)
        .unwrap()
        .into_graph()
        .unwrap();
    let timetable = Rc::new(Timetable::new(engine.top(), graph, &platform).unwrap());
    let before = tracker.events();
    let summary = timetable.preparation_description();
    assert_eq!(summary, timetable.preparation_description());
    assert_eq!(before, tracker.events());
    assert_eq!(clock.tick_now().tick(), 0);
    assert_eq!(timetable.num_graph_nodes_active(), 0);
    let pe = platform.pe("pe0").unwrap();
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert_eq!(pe.machine_ops().checked_operation_total().unwrap(), 0);
    let mem = platform.memory("mem").unwrap();
    assert_eq!((mem.bytes_read(), mem.bytes_written()), (0, 0));
    // Explaining must leave the prepared handles ready for normal dispatch.
    let dispatcher: Rc<dyn Dispatch> = timetable.clone();
    platform.attach_dispatcher(&dispatcher);
    engine.run().unwrap();
    timetable.check_tasks_complete().unwrap();
    assert_eq!(pe.memory_traffic_bytes(), (128, 64));
}

#[test]
fn explanation_shows_implicit_full_tiles_and_clamps_oversized_steps() {
    let full = TIMETABLE.replace("    tiling: { sizes: { i: 4 } }\n", "");
    let summary = success(&cli(PLATFORM, &full, &["--explain"]));
    assert!(summary.contains("declared tile sizes: none"), "{summary}");
    assert!(summary.contains("effective tile sizes: i=8"), "{summary}");
    let oversized = TIMETABLE.replace("i: 4", "i: 64");
    let summary = success(&cli(PLATFORM, &oversized, &["--explain"]));
    assert!(summary.contains("declared tile sizes: i=64"), "{summary}");
    assert!(summary.contains("effective tile sizes: i=8"), "{summary}");
}

#[test]
fn explanation_size_is_independent_of_tile_count() {
    let many = TIMETABLE
        .replace("i: 4", "i: 1")
        .replace("shape: [8]", "shape: [1000]");
    let summary = success(&cli(PLATFORM, &many, &["--explain"]));
    let few = success(&cli(PLATFORM, TIMETABLE, &["--explain"]));
    assert_eq!(summary.lines().count(), few.lines().count());
    assert!(summary.len() < 2500, "summary has {} bytes", summary.len());
}

#[test]
fn explain_uses_named_views_and_effective_hardware_overrides() {
    let platform = PLATFORM.replace(
        "sram_bytes: 256",
        "sram_bytes: 512\n      num_active_requests: 3\n      lsu_access_bytes: 16\n      overhead_size_bytes: 9",
    );
    let timetable = TIMETABLE.replace(
        "    tiling: { sizes: { i: 4 } }",
        "    input_views:\n      A: { offsets: [2], shape: [4] }\n      B: { offsets: [1], shape: [4] }\n    output_views:\n      T: { offsets: [3], shape: [4] }\n    tiling: { sizes: { i: 3 } }",
    );
    // Use just the first compute node so its output view need not feed a
    // differently shaped downstream task.
    let mut file = TimetableFile::from_string(&timetable).unwrap();
    file.nodes
        .retain(|node| node.id() != "multiply" && node.id() != "Y" && node.id() != "C");
    file.edges.retain(|edge| {
        edge.from_node_id() != "C"
            && edge.to_node_id() != "multiply"
            && edge.from_node_id() != "multiply"
    });
    let summary = success(&cli(
        &platform,
        &yaml_serde::to_string(&file).unwrap(),
        &["--explain"],
    ));
    for fact in [
        "SRAM capacity: 512 bytes",
        "LSU access: 16 bytes; request slots: 3; protocol overhead: 9 bytes",
        "input A <- A dtype=fp32 shape=[4] offsets=[2]",
        "output T -> T dtype=fp32 shape=[4] offsets=[3]",
        "extents: i=4",
        "effective tile sizes: i=3",
        "read=32 bytes, write=16 bytes",
    ] {
        assert!(summary.contains(fact), "missing {fact}:\n{summary}");
    }
}
