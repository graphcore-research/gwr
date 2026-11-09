// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::Path;
use std::process::Command;

#[test]
fn lists_matching_activity_lanes_and_reports_no_matches() {
    let binary = env!("CARGO_BIN_EXE_gwr-timetable-entities");
    for option in [
        "--filter-regex",
        "--regex",
        "--perfetto-filter-regex",
        "--binary-filter-regex",
        "--stdout-filter-regex",
    ] {
        let output = Command::new(binary)
            .args(example_paths())
            .args([option, "^top::pe0($|::lane::)"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{option}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("Filter regex: \"^top::pe0($|::lane::)\""));
        assert!(stdout.contains("MATCH    top::pe0::lane::compute::vector_add::0"));
        assert!(!stdout.contains("top::time"));
        assert!(!stdout.contains("top::platform"));
        assert!(stdout.contains("Matched "));
        assert!(stdout.contains(" entities."));
    }

    let output = Command::new(binary)
        .args(example_paths())
        .args(["--regex", "^missing$"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Matched 0 of ")
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("matched no entities")
    );
}

#[test]
fn a_lane_filter_excludes_pe_parents_and_unrelated_entities() {
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-timetable-entities"))
        .args(example_paths())
        .args(["--regex", "^top::pe.*::lane::", "--show-all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("MATCH    top::pe0::lane::compute::vector_add::0"));
    for name in ["top::pe0", "top::time", "top::platform"] {
        assert!(stdout.contains(&format!("NO MATCH {name}\n")), "{name}");
    }
}

#[test]
fn only_one_filter_option_is_accepted() {
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-timetable-entities"))
        .args(["--regex", "pe0", "--stdout-filter-regex", "pe0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn invalid_perfetto_regex_reports_the_option_without_panicking() {
    let output = Command::new(env!("CARGO_BIN_EXE_gwr-timetable"))
        .args(example_paths())
        .args(["--perfetto", "--perfetto-filter-regex", "[invalid"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--perfetto-filter-regex"));
    assert!(stderr.contains("unclosed character class"));
    assert!(!stderr.contains("panicked"));
}

fn example_paths() -> [String; 4] {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    [
        "--platform".into(),
        root.join("examples/compute_region_platform.yaml")
            .display()
            .to_string(),
        "--timetable".into(),
        root.join("examples/fused_region.yaml")
            .display()
            .to_string(),
    ]
}
