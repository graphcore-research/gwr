// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;
use std::process::Command;

use gwr_code_coverage::{
    CoverageReport, DEFAULT_CONTEXT_LINES, RenderOptions, coverage_did_not_decrease,
    render_markdown_with_options,
};
use tempfile::TempDir;

const BEFORE_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/before-summary.json"
);
const AFTER_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/after-summary.json"
);

fn render_markdown(
    before: &CoverageReport,
    after: &CoverageReport,
    show_all_files: bool,
) -> String {
    render_markdown_with_options(
        before,
        after,
        &RenderOptions {
            show_all_files,
            context: DEFAULT_CONTEXT_LINES,
            ..RenderOptions::default()
        },
    )
}

#[test]
fn reports_total_and_file_deltas() {
    let before = CoverageReport::from_json(include_str!("fixtures/before-summary.json")).unwrap();
    let after = CoverageReport::from_json(include_str!("fixtures/after-summary.json")).unwrap();

    let report = render_markdown(&before, &after, false);

    assert!(report.contains("## Files: showing 3/4 changed files"));
    assert!(report.contains("| Lines | 8/10 (80.00%) | 11/12 (91.67%) | +3 / +2 (+11.67%) |"));
    assert!(report.contains("| Functions | 2/4 (50.00%) | 4/4 (100.00%) | +2 / 0 (+50.00%) |"));
    assert!(report.contains("| Regions | 7/10 (70.00%) | 10/12 (83.33%) | +3 / +2 (+13.33%) |"));
    assert!(!report.contains("| Combined |"));
    assert!(report.contains("| File | Lines Before | Lines After | Lines Delta | Functions Before | Functions After | Functions Delta | Regions Before | Regions After | Regions Delta |"));
    assert!(
        report.contains("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
    );
    assert!(
        report.contains(
            "| `src/lib.rs` | 8/10 (80.00%) | 9/10 (90.00%) | +1 / 0 (+10.00%) | 2/4 (50.00%) | 4/4 (100.00%) | +2 / 0 (+50.00%) | 7/10 (70.00%) | 9/10 (90.00%) | +2 / 0 (+20.00%) |"
        )
    );
    assert!(report.contains(
        "| `src/new.rs` | _ | 2/2 (100.00%) | _ | _ | 1/1 (100.00%) | _ | _ | 2/2 (100.00%) | _ |"
    ));
    assert!(
        report.contains(
            "| `src/removed.rs` | 1/1 (100.00%) | _ | _ | 1/1 (100.00%) | _ | _ | 1/1 (100.00%) | _ | _ |"
        )
    );
    assert!(!report.contains("src/unchanged.rs"));
}

#[test]
fn reports_whether_any_total_coverage_metric_decreased() {
    let before = CoverageReport::from_json(include_str!("fixtures/before-summary.json")).unwrap();
    let after = CoverageReport::from_json(include_str!("fixtures/after-summary.json")).unwrap();

    assert!(coverage_did_not_decrease(&before, &after));
    assert!(!coverage_did_not_decrease(&after, &before));
}

#[test]
fn coverage_decreases_when_any_metric_decreases() {
    let before = CoverageReport::from_json(
        r#"{
          "data": [{
            "files": [],
            "totals": {
              "lines": { "count": 10, "covered": 10, "percent": 100.0 },
              "functions": { "count": 10, "covered": 5, "percent": 50.0 },
              "regions": { "count": 10, "covered": 5, "percent": 50.0 }
            }
          }]
        }"#,
    )
    .unwrap();
    let after = CoverageReport::from_json(
        r#"{
          "data": [{
            "files": [],
            "totals": {
              "lines": { "count": 10, "covered": 9, "percent": 90.0 },
              "functions": { "count": 10, "covered": 10, "percent": 100.0 },
              "regions": { "count": 10, "covered": 10, "percent": 100.0 }
            }
          }]
        }"#,
    )
    .unwrap();

    assert!(!coverage_did_not_decrease(&before, &after));
}

#[test]
fn diff_coverage_exits_success_when_no_total_coverage_metric_drops() {
    let status = Command::new(env!("CARGO_BIN_EXE_diff-coverage"))
        .arg(BEFORE_FIXTURE)
        .arg(AFTER_FIXTURE)
        .status()
        .unwrap();

    assert!(status.success());
}

#[test]
fn diff_coverage_exits_failure_when_any_total_coverage_metric_drops() {
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
        "| `src/unchanged.rs` | 3/3 (100.00%) | 3/3 (100.00%) | 0 / 0 (0.00%) | 1/1 (100.00%) | 1/1 (100.00%) | 0 / 0 (0.00%) | 3/3 (100.00%) | 3/3 (100.00%) | 0 / 0 (0.00%) |"
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
            context: 3,
        },
    );

    assert!(report.contains("| Before | `before-summary.json` | `/tmp/before-checkout` |"));
    assert!(report.contains("| After | `after-summary.json` | `/tmp/after-checkout` |"));
    assert!(
        report.contains(
            "| `src/lib.rs` | 8/10 (80.00%) | 9/10 (90.00%) | +1 / 0 (+10.00%) | 1/1 (100.00%) | 1/1 (100.00%) | 0 / 0 (0.00%) | 8/10 (80.00%) | 9/10 (90.00%) | +1 / 0 (+10.00%) |"
        )
    );
}

#[test]
fn absolute_prefixes_are_inferred_and_reported() {
    let before = CoverageReport::from_json(
        r#"{
          "data": [{
            "files": [{
              "filename": "/tmp/before-checkout/gwr-code-coverage/src/lib.rs",
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
              "filename": "/tmp/after-checkout/gwr-code-coverage/src/lib.rs",
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

    assert!(report.contains("| Before | _ | `/tmp/before-checkout/gwr-code-coverage/src` |"));
    assert!(report.contains("| After | _ | `/tmp/after-checkout/gwr-code-coverage/src` |"));
    assert!(
        report.contains("| `lib.rs` | 2/4 (50.00%) | 3/4 (75.00%) | +1 / 0 (+25.00%) | 1/1 (100.00%) | 1/1 (100.00%) | 0 / 0 (0.00%) | 2/4 (50.00%) | 3/4 (75.00%) | +1 / 0 (+25.00%) |")
    );
}

#[test]
fn full_reports_include_annotated_gained_and_lost_line_coverage() {
    let temp_dir = TempDir::new().unwrap();
    let before_dir = temp_dir.path().join("before");
    let after_dir = temp_dir.path().join("after");
    let before_source = before_dir.join("src/lib.rs");
    let after_source = after_dir.join("src/lib.rs");
    fs::create_dir_all(before_source.parent().unwrap()).unwrap();
    fs::create_dir_all(after_source.parent().unwrap()).unwrap();
    fs::write(
        &before_source,
        "fn kept() {}\nfn gained() {}\nfn lost() {}\n",
    )
    .unwrap();
    fs::write(
        &after_source,
        "fn kept() {}\nfn gained() {}\nfn lost() {}\nfn new_gain() {}\n",
    )
    .unwrap();

    let before = CoverageReport::from_json(&full_report_json(
        &before_source.display().to_string(),
        &[[1, 4], [2, 0], [3, 7]],
        2,
        3,
    ))
    .unwrap();
    let after = CoverageReport::from_json(&full_report_json(
        &after_source.display().to_string(),
        &[[1, 4], [2, 3], [3, 0], [4, 1]],
        3,
        4,
    ))
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            before_prefix: Some(before_dir.display().to_string()),
            after_prefix: Some(after_dir.display().to_string()),
            ..RenderOptions::default()
        },
    );

    assert!(report.contains("## Line Coverage Changes"));
    assert!(report.contains(
        "Rows are shown as `before-line -> after-line [before-hits -> after-hits] | source`"
    ));
    assert!(report.contains("### `src/lib.rs`"));
    assert!(report.contains("@@ -1,3 +1,4 @@"));
    assert!(report.contains("  1 -> 1 [4 -> 4] | fn kept() {}"));
    assert!(report.contains("+ 2 -> 2 [0 -> 3] | fn gained() {}"));
    assert!(report.contains("- 3 -> 3 [7 -> 0] | fn lost() {}"));
    assert!(report.contains("+ _ -> 4 [_ -> 1] | fn new_gain() {}"));
}

#[test]
fn full_reports_honor_requested_line_context() {
    let temp_dir = TempDir::new().unwrap();
    let before_dir = temp_dir.path().join("before");
    let after_dir = temp_dir.path().join("after");
    let before_source = before_dir.join("src/lib.rs");
    let after_source = after_dir.join("src/lib.rs");
    fs::create_dir_all(before_source.parent().unwrap()).unwrap();
    fs::create_dir_all(after_source.parent().unwrap()).unwrap();
    let source = "line 1\nline 2\nline 3\nline 4\nline 5\n";
    fs::write(&before_source, source).unwrap();
    fs::write(&after_source, source).unwrap();

    let before = CoverageReport::from_json(&full_report_json(
        &before_source.display().to_string(),
        &[[1, 1], [2, 1], [3, 0], [4, 1], [5, 1]],
        4,
        5,
    ))
    .unwrap();
    let after = CoverageReport::from_json(&full_report_json(
        &after_source.display().to_string(),
        &[[1, 1], [2, 1], [3, 5], [4, 1], [5, 1]],
        5,
        5,
    ))
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            before_prefix: Some(before_dir.display().to_string()),
            after_prefix: Some(after_dir.display().to_string()),
            context: 1,
            ..RenderOptions::default()
        },
    );

    assert!(report.contains("@@ -2,4 +2,4 @@"));
    assert!(report.contains("  2 -> 2 [1 -> 1] | line 2"));
    assert!(report.contains("+ 3 -> 3 [0 -> 5] | line 3"));
    assert!(report.contains("  4 -> 4 [1 -> 1] | line 4"));
    assert!(!report.contains("  1 -> 1 [1 -> 1] | line 1"));
    assert!(!report.contains("  5 -> 5 [1 -> 1] | line 5"));
}

#[test]
fn full_reports_align_multi_digit_line_numbers() {
    let temp_dir = TempDir::new().unwrap();
    let before_dir = temp_dir.path().join("before");
    let after_dir = temp_dir.path().join("after");
    let before_source = before_dir.join("src/lib.rs");
    let after_source = after_dir.join("src/lib.rs");
    fs::create_dir_all(before_source.parent().unwrap()).unwrap();
    fs::create_dir_all(after_source.parent().unwrap()).unwrap();
    let source = (1..=12)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&before_source, format!("{source}\n")).unwrap();
    fs::write(&after_source, format!("{source}\n")).unwrap();

    let before = CoverageReport::from_json(&full_report_json(
        &before_source.display().to_string(),
        &[[9, 1], [10, 0], [11, 1]],
        2,
        3,
    ))
    .unwrap();
    let after = CoverageReport::from_json(&full_report_json(
        &after_source.display().to_string(),
        &[[9, 1], [10, 12], [11, 1]],
        3,
        3,
    ))
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            before_prefix: Some(before_dir.display().to_string()),
            after_prefix: Some(after_dir.display().to_string()),
            context: 1,
            ..RenderOptions::default()
        },
    );

    assert!(report.contains("   9 ->  9 [1 -> 1 ] | line 9"));
    assert!(report.contains("+ 10 -> 10 [0 -> 12] | line 10"));
    assert!(report.contains("  11 -> 11 [1 -> 1 ] | line 11"));
}

#[test]
fn full_reports_align_changed_source_line_numbers() {
    let temp_dir = TempDir::new().unwrap();
    let before_dir = temp_dir.path().join("before");
    let after_dir = temp_dir.path().join("after");
    let before_source = before_dir.join("src/lib.rs");
    let after_source = after_dir.join("src/lib.rs");
    fs::create_dir_all(before_source.parent().unwrap()).unwrap();
    fs::create_dir_all(after_source.parent().unwrap()).unwrap();
    fs::write(&before_source, "fn first() {}\nfn target() {}\n").unwrap();
    fs::write(
        &after_source,
        "fn first() {}\nfn inserted() {}\nfn target() {}\n",
    )
    .unwrap();

    let before = CoverageReport::from_json(&full_report_json(
        &before_source.display().to_string(),
        &[[1, 1], [2, 0]],
        1,
        2,
    ))
    .unwrap();
    let after = CoverageReport::from_json(&full_report_json(
        &after_source.display().to_string(),
        &[[1, 1], [2, 0], [3, 8]],
        2,
        3,
    ))
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            before_prefix: Some(before_dir.display().to_string()),
            after_prefix: Some(after_dir.display().to_string()),
            context: 1,
            ..RenderOptions::default()
        },
    );

    assert!(report.contains("+ 2 -> 3 [0 -> 8]"));
    assert!(report.contains("| fn target() {}"));
    assert!(!report.contains("+ 2 -> 2 [0 -> 8]"));
}

#[test]
fn full_reports_align_inserted_lines_around_modified_source() {
    let temp_dir = TempDir::new().unwrap();
    let before_dir = temp_dir.path().join("before");
    let after_dir = temp_dir.path().join("after");
    let before_source = before_dir.join("src/lib.rs");
    let after_source = after_dir.join("src/lib.rs");
    fs::create_dir_all(before_source.parent().unwrap()).unwrap();
    fs::create_dir_all(after_source.parent().unwrap()).unwrap();
    fs::write(
        &before_source,
        "fn first() {}\nfn target_old_value() {}\nfn last() {}\n",
    )
    .unwrap();
    fs::write(
        &after_source,
        "fn first() {}\nfn inserted() {}\nfn target_new_value() {}\nfn last() {}\n",
    )
    .unwrap();

    let before = CoverageReport::from_json(&full_report_json(
        &before_source.display().to_string(),
        &[[1, 1], [2, 0], [3, 1]],
        2,
        3,
    ))
    .unwrap();
    let after = CoverageReport::from_json(&full_report_json(
        &after_source.display().to_string(),
        &[[1, 1], [2, 0], [3, 6], [4, 1]],
        3,
        4,
    ))
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            before_prefix: Some(before_dir.display().to_string()),
            after_prefix: Some(after_dir.display().to_string()),
            context: 1,
            ..RenderOptions::default()
        },
    );

    assert!(report.contains("+ _ -> 2 [_ -> 0] | fn inserted() {}"));
    assert!(report.contains("+ 2 -> 3 [0 -> 6]"));
    assert!(report.contains("| fn target_new_value() {}"));
    assert!(!report.contains("+ 2 -> 2 [0 -> 6]"));
}

#[test]
fn full_reports_render_new_uncovered_files_in_line_changes() {
    let temp_dir = TempDir::new().unwrap();
    let after_dir = temp_dir.path().join("after");
    let after_source = after_dir.join("src/foo.rs");
    fs::create_dir_all(after_source.parent().unwrap()).unwrap();
    fs::write(&after_source, "fn foo() {}\nfn unused() {}\n").unwrap();

    let before = CoverageReport::from_json(&empty_report_json()).unwrap();
    let after = CoverageReport::from_json(&full_report_json(
        &after_source.display().to_string(),
        &[[1, 0], [2, 0]],
        0,
        2,
    ))
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            after_prefix: Some(after_dir.display().to_string()),
            context: 0,
            ..RenderOptions::default()
        },
    );

    assert!(report.contains("### `src/foo.rs` changes"));
    assert!(report.contains("+ _ -> 1 [_ -> 0] | fn foo() {}"));
    assert!(report.contains("+ _ -> 2 [_ -> 0] | fn unused() {}"));
}

#[test]
fn full_reports_render_removed_source_lines_from_before_source() {
    let temp_dir = TempDir::new().unwrap();
    let before_dir = temp_dir.path().join("before");
    let after_dir = temp_dir.path().join("after");
    let before_source = before_dir.join("src/lib.rs");
    let after_source = after_dir.join("src/lib.rs");
    fs::create_dir_all(before_source.parent().unwrap()).unwrap();
    fs::create_dir_all(after_source.parent().unwrap()).unwrap();
    fs::write(
        &before_source,
        "fn first() {}\nfn removed() {}\nfn kept() {}\n",
    )
    .unwrap();
    fs::write(&after_source, "fn first() {}\nfn kept() {}\n").unwrap();

    let before = CoverageReport::from_json(&full_report_json(
        &before_source.display().to_string(),
        &[[1, 1], [2, 7], [3, 1]],
        3,
        3,
    ))
    .unwrap();
    let after = CoverageReport::from_json(&full_report_json(
        &after_source.display().to_string(),
        &[[1, 1], [2, 1]],
        2,
        2,
    ))
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            before_prefix: Some(before_dir.display().to_string()),
            after_prefix: Some(after_dir.display().to_string()),
            context: 1,
            ..RenderOptions::default()
        },
    );

    assert!(report.contains("- 2 -> _ [7 -> _] | fn removed() {}"));
    assert!(report.contains("  3 -> 2 [1 -> 1] | fn kept() {}"));
}

#[test]
fn full_reports_do_not_extend_segments_past_same_line_boundaries() {
    let temp_dir = TempDir::new().unwrap();
    let before_dir = temp_dir.path().join("before");
    let after_dir = temp_dir.path().join("after");
    let before_source = before_dir.join("src/lib.rs");
    let after_source = after_dir.join("src/lib.rs");
    fs::create_dir_all(before_source.parent().unwrap()).unwrap();
    fs::create_dir_all(after_source.parent().unwrap()).unwrap();
    let source = "covered();\ngained();\nstop();\n";
    fs::write(&before_source, source).unwrap();
    fs::write(&after_source, source).unwrap();

    let before = CoverageReport::from_json(&full_report_json_with_segments(
        &before_source.display().to_string(),
        &[
            "[1,1,4,true,true,false]",
            "[1,10,0,true,false,false]",
            "[3,1,0,true,true,false]",
        ],
        1,
        3,
    ))
    .unwrap();
    let after = CoverageReport::from_json(&full_report_json_with_segments(
        &after_source.display().to_string(),
        &[
            "[1,1,4,true,true,false]",
            "[1,10,0,true,false,false]",
            "[2,1,7,true,true,false]",
            "[3,1,0,true,true,false]",
        ],
        2,
        3,
    ))
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            before_prefix: Some(before_dir.display().to_string()),
            after_prefix: Some(after_dir.display().to_string()),
            context: 0,
            ..RenderOptions::default()
        },
    );

    assert!(report.contains("+ 2 -> 2 [0 -> 7] | gained();"));
}

#[test]
fn full_reports_explain_summary_line_changes_without_source_line_flips() {
    let temp_dir = TempDir::new().unwrap();
    let before_dir = temp_dir.path().join("before");
    let after_dir = temp_dir.path().join("after");
    let before_source = before_dir.join("src/lib.rs");
    let after_source = after_dir.join("src/lib.rs");
    fs::create_dir_all(before_source.parent().unwrap()).unwrap();
    fs::create_dir_all(after_source.parent().unwrap()).unwrap();
    fs::write(&before_source, "already_covered();\n").unwrap();
    fs::write(&after_source, "already_covered();\n").unwrap();

    let before = CoverageReport::from_json(&full_report_json(
        &before_source.display().to_string(),
        &[[1, 1]],
        1,
        2,
    ))
    .unwrap();
    let after = CoverageReport::from_json(&full_report_json(
        &after_source.display().to_string(),
        &[[1, 2]],
        2,
        2,
    ))
    .unwrap();

    let report = render_markdown_with_options(
        &before,
        &after,
        &RenderOptions {
            before_prefix: Some(before_dir.display().to_string()),
            after_prefix: Some(after_dir.display().to_string()),
            ..RenderOptions::default()
        },
    );

    assert!(report.contains(
        "No source-line covered/uncovered changes were found in the llvm-cov segment data."
    ));
    assert!(report.contains("- `src/lib.rs`: +1 covered lines"));
}

#[test]
fn summary_only_reports_do_not_include_line_coverage_changes_section() {
    let before = CoverageReport::from_json(include_str!("fixtures/before-summary.json")).unwrap();
    let after = CoverageReport::from_json(include_str!("fixtures/after-summary.json")).unwrap();

    let report = render_markdown(&before, &after, false);

    assert!(!report.contains("## Line Coverage Changes"));
}

fn full_report_json(filename: &str, line_counts: &[[u64; 2]], covered: u64, count: u64) -> String {
    let segments = line_counts
        .iter()
        .map(|[line, count]| format!("[{line},1,{count},true,true,false]"))
        .collect::<Vec<_>>();

    full_report_json_with_segments(filename, &segments, covered, count)
}

fn empty_report_json() -> String {
    r#"{
          "data": [{
            "files": [],
            "totals": {
              "lines": { "count": 0, "covered": 0, "percent": 0.0 },
              "functions": { "count": 0, "covered": 0, "percent": 0.0 },
              "regions": { "count": 0, "covered": 0, "percent": 0.0 }
            }
          }],
          "type": "llvm.coverage.json.export",
          "version": "2.0.1"
        }"#
    .to_string()
}

fn full_report_json_with_segments(
    filename: &str,
    segments: &[impl AsRef<str>],
    covered: u64,
    count: u64,
) -> String {
    let segments = segments
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(",");

    format!(
        r#"{{
          "data": [{{
            "files": [{{
              "filename": {filename:?},
              "segments": [{segments}],
              "summary": {{
                "lines": {{ "count": {count}, "covered": {covered}, "percent": 0.0 }},
                "functions": {{ "count": 1, "covered": 1, "percent": 100.0 }},
                "regions": {{ "count": {count}, "covered": {covered}, "percent": 0.0 }}
              }}
            }}],
            "totals": {{
              "lines": {{ "count": {count}, "covered": {covered}, "percent": 0.0 }},
              "functions": {{ "count": 1, "covered": 1, "percent": 100.0 }},
              "regions": {{ "count": {count}, "covered": {covered}, "percent": 0.0 }}
            }}
          }}],
          "type": "llvm.coverage.json.export",
          "version": "2.0.1"
        }}"#
    )
}
