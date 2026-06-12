// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::process::Command;

use gwr_tools::{
    CoverageReport, RenderOptions, combined_total_coverage_delta,
    combined_total_coverage_is_non_negative, render_markdown, render_markdown_with_options,
};

const BEFORE_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/before-summary.json"
);
const AFTER_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/after-summary.json"
);

#[test]
fn reports_total_and_file_deltas() {
    let before = CoverageReport::from_json(include_str!("fixtures/before-summary.json")).unwrap();
    let after = CoverageReport::from_json(include_str!("fixtures/after-summary.json")).unwrap();

    let report = render_markdown(&before, &after, false);

    assert!(report.contains("## Files: showing 3/4 changed files"));
    assert!(report.contains("| Combined | 17/24 (70.83%) | 25/28 (89.29%) | +8 / +4 (+18.45%) |"));
    assert!(report.contains("| Lines | 8/10 (80.00%) | 11/12 (91.67%) | +3 / +2 (+11.67%) |"));
    assert!(report.contains("| Functions | 2/4 (50.00%) | 4/4 (100.00%) | +2 / 0 (+50.00%) |"));
    assert!(report.contains("| Regions | 7/10 (70.00%) | 10/12 (83.33%) | +3 / +2 (+13.33%) |"));
    assert!(report.contains("| File | Combined Before | Combined After | Combined Delta | Lines Before | Lines After | Lines Delta | Functions Before | Functions After | Functions Delta | Regions Before | Regions After | Regions Delta |"));
    assert!(report.contains("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"));
    assert!(
        report.contains(
            "| `src/lib.rs` | 17/24 (70.83%) | 22/24 (91.67%) | +5 / 0 (+20.83%) | 8/10 (80.00%) | 9/10 (90.00%) | +1 / 0 (+10.00%) | 2/4 (50.00%) | 4/4 (100.00%) | +2 / 0 (+50.00%) | 7/10 (70.00%) | 9/10 (90.00%) | +2 / 0 (+20.00%) |"
        )
    );
    assert!(report.contains(
        "| `src/new.rs` | n/a | 5/5 (100.00%) | n/a | n/a | 2/2 (100.00%) | n/a | n/a | 1/1 (100.00%) | n/a | n/a | 2/2 (100.00%) | n/a |"
    ));
    assert!(
        report.contains(
            "| `src/removed.rs` | 3/3 (100.00%) | n/a | n/a | 1/1 (100.00%) | n/a | n/a | 1/1 (100.00%) | n/a | n/a | 1/1 (100.00%) | n/a | n/a |"
        )
    );
    assert!(!report.contains("src/unchanged.rs"));
}

#[test]
fn reports_combined_total_coverage_delta() {
    let before = CoverageReport::from_json(include_str!("fixtures/before-summary.json")).unwrap();
    let after = CoverageReport::from_json(include_str!("fixtures/after-summary.json")).unwrap();

    assert!(
        (combined_total_coverage_delta(&before, &after) - 18.452_380_952_380_963).abs() < 1e-12
    );
    assert!(combined_total_coverage_is_non_negative(&before, &after));
    assert!(!combined_total_coverage_is_non_negative(&after, &before));
}

#[test]
fn diff_coverage_exits_success_when_combined_total_coverage_does_not_drop() {
    let status = Command::new(env!("CARGO_BIN_EXE_diff-coverage"))
        .arg(BEFORE_FIXTURE)
        .arg(AFTER_FIXTURE)
        .status()
        .unwrap();

    assert!(status.success());
}

#[test]
fn diff_coverage_exits_failure_when_combined_total_coverage_drops() {
    let status = Command::new(env!("CARGO_BIN_EXE_diff-coverage"))
        .arg(AFTER_FIXTURE)
        .arg(BEFORE_FIXTURE)
        .status()
        .unwrap();

    assert!(!status.success());
}

#[test]
fn can_include_unchanged_files() {
    let before = CoverageReport::from_json(include_str!("fixtures/before-summary.json")).unwrap();
    let after = CoverageReport::from_json(include_str!("fixtures/after-summary.json")).unwrap();

    let report = render_markdown(&before, &after, true);

    assert!(report.contains("## Files: showing all"));
    assert!(report.contains(
        "| `src/unchanged.rs` | 7/7 (100.00%) | 7/7 (100.00%) | 0 / 0 (0.00%) | 3/3 (100.00%) | 3/3 (100.00%) | 0 / 0 (0.00%) | 1/1 (100.00%) | 1/1 (100.00%) | 0 / 0 (0.00%) | 3/3 (100.00%) | 3/3 (100.00%) | 0 / 0 (0.00%) |"
    ));
}

#[test]
fn explicit_prefixes_are_removed_before_matching_files() {
    let before = CoverageReport::from_json(
        r#"{
          "data": [{
            "files": [{
              "filename": "/tmp/before-checkout/src/lib.rs",
              "summary": {
                "lines": { "count": 10, "covered": 8, "percent": 80.0 },
                "functions": { "count": 1, "covered": 1, "percent": 100.0 },
                "regions": { "count": 10, "covered": 8, "percent": 80.0 }
              }
            }],
            "totals": {
              "lines": { "count": 10, "covered": 8, "percent": 80.0 },
              "functions": { "count": 1, "covered": 1, "percent": 100.0 },
              "regions": { "count": 10, "covered": 8, "percent": 80.0 }
            }
          }]
        }"#,
    )
    .unwrap();
    let after = CoverageReport::from_json(
        r#"{
          "data": [{
            "files": [{
              "filename": "/tmp/after-checkout/src/lib.rs",
              "summary": {
                "lines": { "count": 10, "covered": 9, "percent": 90.0 },
                "functions": { "count": 1, "covered": 1, "percent": 100.0 },
                "regions": { "count": 10, "covered": 9, "percent": 90.0 }
              }
            }],
            "totals": {
              "lines": { "count": 10, "covered": 9, "percent": 90.0 },
              "functions": { "count": 1, "covered": 1, "percent": 100.0 },
              "regions": { "count": 10, "covered": 9, "percent": 90.0 }
            }
          }]
        }"#,
    )
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            show_all_files: false,
            before_path: Some("before-summary.json".to_string()),
            after_path: Some("after-summary.json".to_string()),
            before_prefix: Some("/tmp/before-checkout".to_string()),
            after_prefix: Some("/tmp/after-checkout/".to_string()),
        },
    );

    assert!(report.contains("| Before | `before-summary.json` | `/tmp/before-checkout` |"));
    assert!(report.contains("| After | `after-summary.json` | `/tmp/after-checkout` |"));
    assert!(
        report.contains(
            "| `src/lib.rs` | 17/21 (80.95%) | 19/21 (90.48%) | +2 / 0 (+9.52%) | 8/10 (80.00%) | 9/10 (90.00%) | +1 / 0 (+10.00%) |"
        )
    );
}

#[test]
fn absolute_prefixes_are_inferred_and_reported() {
    let before = CoverageReport::from_json(
        r#"{
          "data": [{
            "files": [{
              "filename": "/tmp/before-checkout/gwr-tools/src/lib.rs",
              "summary": {
                "lines": { "count": 4, "covered": 2, "percent": 50.0 },
                "functions": { "count": 1, "covered": 1, "percent": 100.0 },
                "regions": { "count": 4, "covered": 2, "percent": 50.0 }
              }
            }],
            "totals": {
              "lines": { "count": 4, "covered": 2, "percent": 50.0 },
              "functions": { "count": 1, "covered": 1, "percent": 100.0 },
              "regions": { "count": 4, "covered": 2, "percent": 50.0 }
            }
          }]
        }"#,
    )
    .unwrap();
    let after = CoverageReport::from_json(
        r#"{
          "data": [{
            "files": [{
              "filename": "/tmp/after-checkout/gwr-tools/src/lib.rs",
              "summary": {
                "lines": { "count": 4, "covered": 3, "percent": 75.0 },
                "functions": { "count": 1, "covered": 1, "percent": 100.0 },
                "regions": { "count": 4, "covered": 3, "percent": 75.0 }
              }
            }],
            "totals": {
              "lines": { "count": 4, "covered": 3, "percent": 75.0 },
              "functions": { "count": 1, "covered": 1, "percent": 100.0 },
              "regions": { "count": 4, "covered": 3, "percent": 75.0 }
            }
          }]
        }"#,
    )
    .unwrap();

    let report = render_markdown(&before, &after, false);

    assert!(report.contains("| Before | n/a | `/tmp/before-checkout/gwr-tools/src` |"));
    assert!(report.contains("| After | n/a | `/tmp/after-checkout/gwr-tools/src` |"));
    assert!(
        report.contains("| `lib.rs` | 5/9 (55.56%) | 7/9 (77.78%) | +2 / 0 (+22.22%) | 2/4 (50.00%) | 3/4 (75.00%) | +1 / 0 (+25.00%) |")
    );
}
