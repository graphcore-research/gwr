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
        .arg("--error-mermaid")
        .arg(dir.path().join("error.mmd"))
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
            assert_eq!(normal.status.code(), checked.status.code());
            assert_eq!(
                String::from_utf8_lossy(&normal.stderr),
                String::from_utf8_lossy(&checked.stderr)
            );
        }
    }
}

#[test]
fn check_and_explain_validate_task_admission_before_execution() {
    for (platform, timetable, expected) in [
        (
            PLATFORM.to_owned(),
            TIMETABLE.replace("add:", "unknown_op:"),
            "unsupported capability",
        ),
        (
            PLATFORM.replace("sram_bytes: 256", "sram_bytes: 40"),
            TIMETABLE.to_owned(),
            "requires 48 SRAM bytes, available 40",
        ),
        (
            PLATFORM.replace("capacity_bytes: 1MiB", "capacity_bytes: 4KiB"),
            TIMETABLE.to_owned(),
            "unmapped touched bytes",
        ),
    ] {
        for flag in ["--check", "--explain"] {
            let checked = cli(&platform, &timetable, &[flag]);
            assert!(!checked.status.success());
            let error = String::from_utf8_lossy(&checked.stderr);
            assert!(error.contains("Node 'add' on PE 'pe0'"), "{error}");
            assert!(error.contains(expected), "{error}");
        }
    }
}

#[test]
fn ordinary_cli_validates_later_tasks_before_issuing_memory_requests() {
    let timetable = TIMETABLE.replace("mul:", "unknown_op:");
    let output = cli(PLATFORM, &timetable, &["--stdout-level", "trace"]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("Node 'multiply' on PE 'pe0'"), "{error}");
    assert!(error.contains("unsupported capability"), "{error}");
    let log = String::from_utf8_lossy(&output.stdout);
    assert!(!log.contains("MemoryAccess:"), "{log}");
}

#[test]
fn preparation_modes_skip_activity_export() {
    let dir = tempdir().unwrap();
    let activity = dir.path().join("activity.json");
    let path = activity.to_str().unwrap();
    for flag in ["--check", "--explain"] {
        success(&cli(PLATFORM, TIMETABLE, &[flag, "--activity-out", path]));
        assert!(!activity.exists());
    }
    success(&cli(PLATFORM, TIMETABLE, &["--activity-out", path]));
    assert!(activity.is_file());
}

#[test]
fn preparation_modes_conflict_with_roofline_analysis() {
    for flag in ["--check", "--explain"] {
        let output = cli(PLATFORM, TIMETABLE, &[flag, "--roofline-analyze"]);
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("cannot be used with"), "{error}");
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
    timetable.preflight().unwrap();
    let summary = timetable.preparation_description().unwrap();
    assert_eq!(summary, timetable.preparation_description().unwrap());
    assert_eq!(before, tracker.events());
    assert_eq!(clock.tick_now().tick(), 0);
    assert_eq!(timetable.num_graph_nodes_active(), 0);
    let pe = platform.pe("pe0").unwrap();
    assert_eq!(pe.memory_traffic_bytes(), (0, 0));
    assert_eq!(pe.peak_sram_bytes(), 0);
    assert_eq!(pe.machine_ops().checked_operation_total().unwrap(), 0);
    let mem = platform.memory("mem").unwrap();
    assert_eq!((mem.bytes_read(), mem.bytes_written()), (0, 0));
    // The timetable can still be dispatched after preflight and explanation.
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
        "sram_bytes: 512\n      num_active_requests: 3\n      overhead_size_bytes: 9\n      lsu_access_bytes: 16",
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
        edge.from != "C" && !edge.to.starts_with("multiply.") && !edge.from.starts_with("multiply.")
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
