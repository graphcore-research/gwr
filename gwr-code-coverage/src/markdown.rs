// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeSet;
use std::fmt::Write;

use crate::coverage::file::{CoverageFile, CoverageMetric, CoverageSummary, percent_delta};
use crate::coverage::line_coverage_summary_delta;
use crate::coverage::report::CoverageReport;
use crate::source_files::{LineChanges, LineDiffRow, line_change_hunks, line_changes};
use crate::{infer_prefix, normalize_prefix};

pub const DEFAULT_CONTEXT_LINES: usize = 3;
const DEFAULT_NONE_STR: &str = "_";

type FileMetric = (&'static str, fn(&CoverageSummary) -> CoverageMetric);

const FILE_METRICS: [FileMetric; 3] = [
    ("Lines", CoverageSummary::lines),
    ("Functions", CoverageSummary::functions),
    ("Regions", CoverageSummary::regions),
];

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub show_all_files: bool,
    pub before_path: Option<String>,
    pub after_path: Option<String>,
    pub before_prefix: Option<String>,
    pub after_prefix: Option<String>,
    pub context: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            show_all_files: false,
            before_path: None,
            after_path: None,
            before_prefix: None,
            after_prefix: None,
            context: DEFAULT_CONTEXT_LINES,
        }
    }
}

#[must_use]
pub fn render_markdown_with_options(
    before: &CoverageReport,
    after: &CoverageReport,
    options: &RenderOptions,
) -> String {
    let before_prefix = options
        .before_prefix
        .as_deref()
        .map_or_else(|| infer_prefix(before.filenames()), normalize_prefix);
    let after_prefix = options
        .after_prefix
        .as_deref()
        .map_or_else(|| infer_prefix(after.filenames()), normalize_prefix);

    let mut report = String::new();
    report.push_str("# Coverage Delta\n\n");
    report.push_str(&render_coverage_files_section(
        options,
        &before_prefix,
        &after_prefix,
    ));
    report.push_str(&render_totals_section(before.totals(), after.totals()));
    report.push_str(&render_files_section(
        before,
        after,
        options.show_all_files,
        &before_prefix,
        &after_prefix,
    ));
    report.push_str(&render_line_changes_section(
        before,
        after,
        &before_prefix,
        &after_prefix,
        options.context,
    ));

    report
}

fn render_coverage_files_section(
    options: &RenderOptions,
    before_prefix: &str,
    after_prefix: &str,
) -> String {
    let mut section = String::new();
    section.push_str("## Coverage Files\n\n");
    section.push_str("| Report | JSON file | Prefix removed |\n");
    section.push_str("| --- | --- | --- |\n");
    push_coverage_file_row(
        &mut section,
        "Before",
        options.before_path.as_deref(),
        before_prefix,
    );
    push_coverage_file_row(
        &mut section,
        "After",
        options.after_path.as_deref(),
        after_prefix,
    );
    section.push('\n');

    section
}

fn push_coverage_file_row(report: &mut String, label: &str, path: Option<&str>, prefix: &str) {
    let path = path.map_or(DEFAULT_NONE_STR.to_string(), format_path);
    let prefix = if prefix.is_empty() {
        "none".to_string()
    } else {
        format_path(prefix)
    };
    let _ = writeln!(report, "| {label} | {path} | {prefix} |");
}

fn format_path(value: &str) -> String {
    format!("`{}`", escape_markdown_code(value))
}

fn render_totals_section(before: &CoverageSummary, after: &CoverageSummary) -> String {
    let mut section = String::new();
    section.push_str("## Totals\n\n");
    section.push_str("| Metric | Before | After | Delta |\n");
    section.push_str("| --- | ---: | ---: | ---: |\n");
    for (metric, metric_access_fn) in FILE_METRICS {
        push_metric_row(
            &mut section,
            metric,
            metric_access_fn(before),
            metric_access_fn(after),
        );
    }
    section
}

fn push_metric_row(
    report: &mut String,
    metric: &str,
    before: CoverageMetric,
    after: CoverageMetric,
) {
    let _ = writeln!(
        report,
        "| {metric} | {} | {} | {} |",
        format_metric(before),
        format_metric(after),
        format_delta(before, after),
    );
}

fn render_files_section(
    before: &CoverageReport,
    after: &CoverageReport,
    show_all_files: bool,
    before_prefix: &str,
    after_prefix: &str,
) -> String {
    let before_files = before.files(before_prefix);
    let after_files = after.files(after_prefix);
    let filenames = before_files
        .keys()
        .chain(after_files.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    let total_file_count = filenames.len();
    let mut file_rows = Vec::new();
    for filename in filenames {
        let before_summary = before_files.get(&filename).copied();
        let after_summary = after_files.get(&filename).copied();
        let is_unchanged = before_summary
            .zip(after_summary)
            .is_some_and(|(before, after)| before == after);

        if !show_all_files && is_unchanged {
            continue;
        }

        file_rows.push((filename, before_summary, after_summary));
    }

    let mut section = String::new();
    if show_all_files {
        section.push_str("\n## Files: showing all\n\n");
    } else {
        let _ = write!(
            section,
            "\n## Files: showing {}/{} changed files\n\n",
            file_rows.len(),
            total_file_count,
        );
    }

    if file_rows.is_empty() {
        section.push_str("No file-level coverage changes.\n");
    } else {
        push_file_table_header(&mut section);
        for (filename, before_summary, after_summary) in file_rows {
            push_file_row(&mut section, &filename, before_summary, after_summary);
        }
    }

    section
}

fn push_file_table_header(section: &mut String) {
    section.push_str("| File");
    for (metric, _) in FILE_METRICS {
        let _ = write!(
            section,
            " | {metric} Before | {metric} After | {metric} Delta"
        );
    }
    section.push_str(" |\n");

    section.push_str("| ---");
    for _ in 0..(FILE_METRICS.len() * 3) {
        section.push_str(" | ---:");
    }
    section.push_str(" |\n");
}

fn push_file_row(
    report: &mut String,
    filename: &str,
    before: Option<&CoverageSummary>,
    after: Option<&CoverageSummary>,
) {
    let _ = write!(report, "| `{}`", escape_markdown_code(filename));
    for (_, metric_access_fn) in FILE_METRICS {
        let _ = write!(
            report,
            " | {}",
            format_file_metric_group(before, after, metric_access_fn)
        );
    }
    let _ = writeln!(report, " |");
}

fn format_file_metric_group(
    before: Option<&CoverageSummary>,
    after: Option<&CoverageSummary>,
    metric_access_fn: fn(&CoverageSummary) -> CoverageMetric,
) -> String {
    let before_metric = before.map(metric_access_fn);
    let after_metric = after.map(metric_access_fn);

    format!(
        "{} | {} | {}",
        format_optional_metric(before_metric),
        format_optional_metric(after_metric),
        format_optional_delta(before_metric, after_metric),
    )
}

fn format_optional_metric(metric: Option<CoverageMetric>) -> String {
    metric.map_or(DEFAULT_NONE_STR.to_string(), format_metric)
}

fn format_optional_delta(before: Option<CoverageMetric>, after: Option<CoverageMetric>) -> String {
    match (before, after) {
        (Some(before), Some(after)) => format_delta(before, after),
        (None, Some(_)) | (Some(_), None) => DEFAULT_NONE_STR.to_string(),
        (None, None) => unreachable!("file row has at least one metric"),
    }
}

fn render_line_changes_section(
    before: &CoverageReport,
    after: &CoverageReport,
    before_prefix: &str,
    after_prefix: &str,
    context: usize,
) -> String {
    if !before.has_line_details() && !after.has_line_details() {
        return String::new();
    }

    let before_files = before.coverage_files(before_prefix);
    let after_files = after.coverage_files(after_prefix);
    let filenames = before_files
        .keys()
        .chain(after_files.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut section = String::new();
    section.push_str("\n## Line Coverage Changes\n");

    let mut rendered_files = 0;
    let mut summary_only_line_changes = Vec::new();
    for filename in filenames {
        let before_file = before_files.get(&filename).copied();
        let after_file = after_files.get(&filename).copied();
        let before_source_lines = before_file.and_then(CoverageFile::source_lines);
        let after_source_lines = after_file.and_then(CoverageFile::source_lines);
        let changes = line_changes(
            before_file,
            after_file,
            before_source_lines.as_deref(),
            after_source_lines.as_deref(),
        );

        if !changes.has_changes() {
            if let Some(delta) = line_coverage_summary_delta(before_file, after_file) {
                summary_only_line_changes.push((filename, delta));
            }
            continue;
        }

        if rendered_files == 0 {
            let _ = writeln!(
                section,
                "\nRows are shown as `before-line -> after-line [before-hits -> after-hits] | source`; `{DEFAULT_NONE_STR}` means the line or hit count is absent.",
            );
        }
        rendered_files += 1;

        let _ = writeln!(
            section,
            "\n### `{}` changes\n",
            escape_markdown_code(&filename)
        );
        push_line_change_diff(
            &mut section,
            &changes,
            before_source_lines.as_ref(),
            after_source_lines.as_ref(),
            context,
        );
    }

    if rendered_files == 0 {
        if summary_only_line_changes.is_empty() {
            section.push_str("\nNo line-level coverage changes.\n");
        } else {
            section.push_str(
                "\nNo source-line covered/uncovered changes were found in the llvm-cov segment data.\n\n",
            );
            section.push_str(
                "The file summaries still report line coverage changes, which can happen when llvm-cov's summary changes come from instantiation or region data that is already aggregated in the source-line segments:\n\n",
            );
            for (filename, delta) in summary_only_line_changes {
                let _ = writeln!(
                    section,
                    "- `{}`: {} covered lines",
                    escape_markdown_code(&filename),
                    format_signed_count(delta),
                );
            }
        }
    }

    section
}

fn push_line_change_diff(
    section: &mut String,
    changes: &LineChanges,
    before_lines: Option<&Vec<String>>,
    after_lines: Option<&Vec<String>>,
    context: usize,
) {
    let changed_rows = changes.changed_rows();
    if changed_rows.is_empty() {
        return;
    }

    section.push_str("```diff\n");
    for hunk in line_change_hunks(&changed_rows, context, changes.rows.len()) {
        let _ = writeln!(
            section,
            "{}",
            format_hunk_header(&changes.rows[hunk.start..hunk.end])
        );
        let before_line_width = max_line_width(&changes.rows[hunk.start..hunk.end], true);
        let after_line_width = max_line_width(&changes.rows[hunk.start..hunk.end], false);
        let before_hit_count_width = max_hit_count_width(&changes.rows[hunk.start..hunk.end], true);
        let after_hit_count_width = max_hit_count_width(&changes.rows[hunk.start..hunk.end], false);
        for row in &changes.rows[hunk.start..hunk.end] {
            let source_line = source_line(row, before_lines, after_lines);
            let _ = writeln!(
                section,
                "{} {} -> {} {} | {}",
                row.marker,
                format_optional_line(row.aligned_line.before_line_number, before_line_width),
                format_optional_line(row.aligned_line.after_line_number, after_line_width),
                format_hit_counts(
                    row.before_count,
                    row.after_count,
                    before_hit_count_width,
                    after_hit_count_width,
                ),
                source_line,
            );
        }
    }
    section.push_str("```\n");
}

fn max_hit_count_width(rows: &[LineDiffRow], before: bool) -> usize {
    rows.iter()
        .map(|row| {
            if before {
                format_optional_count(row.before_count).len()
            } else {
                format_optional_count(row.after_count).len()
            }
        })
        .max()
        .unwrap_or(1)
}

fn format_hunk_header(rows: &[LineDiffRow]) -> String {
    let before_start = rows
        .iter()
        .find_map(|row| row.aligned_line.before_line_number);
    let before_end = rows
        .iter()
        .rev()
        .find_map(|row| row.aligned_line.before_line_number);
    let after_start = rows
        .iter()
        .find_map(|row| row.aligned_line.after_line_number);
    let after_end = rows
        .iter()
        .rev()
        .find_map(|row| row.aligned_line.after_line_number);

    format!(
        "@@ -{} +{} @@",
        format_line_range(before_start, before_end),
        format_line_range(after_start, after_end),
    )
}

fn format_line_range(start: Option<u64>, end: Option<u64>) -> String {
    match (start, end) {
        (Some(start), Some(end)) if start == end => format!("{start}"),
        (Some(start), Some(end)) => format!("{start},{end}"),
        _ => DEFAULT_NONE_STR.to_string(),
    }
}

fn max_line_width(rows: &[LineDiffRow], before: bool) -> usize {
    rows.iter()
        .filter_map(|row| {
            if before {
                row.aligned_line.before_line_number
            } else {
                row.aligned_line.after_line_number
            }
        })
        .map(|line| line.to_string().len())
        .max()
        .unwrap_or(DEFAULT_NONE_STR.len())
}

fn format_optional_line(line: Option<u64>, width: usize) -> String {
    line.map_or_else(
        || format!("{DEFAULT_NONE_STR:>width$}"),
        |line| format!("{line:>width$}"),
    )
}

fn source_line<'a>(
    row: &LineDiffRow,
    before_lines: Option<&'a Vec<String>>,
    after_lines: Option<&'a Vec<String>>,
) -> &'a str {
    let use_after_line = row.marker == '+' || row.aligned_line.before_line_number.is_none();
    let line = if use_after_line {
        row.aligned_line
            .after_line_number
            .or(row.aligned_line.before_line_number)
    } else {
        row.aligned_line
            .before_line_number
            .or(row.aligned_line.after_line_number)
    };
    let source = if use_after_line {
        after_lines.or(before_lines)
    } else {
        before_lines.or(after_lines)
    };

    line.and_then(|line| source.and_then(|lines| lines.get(line.saturating_sub(1) as usize)))
        .map_or("", String::as_str)
}

fn format_hit_counts(
    before_count: Option<u64>,
    after_count: Option<u64>,
    before_width: usize,
    after_width: usize,
) -> String {
    let before_count = format_optional_count(before_count);
    let after_count = format_optional_count(after_count);
    format!("[{before_count:>before_width$} -> {after_count:<after_width$}]")
}

fn format_optional_count(count: Option<u64>) -> String {
    count.map_or(DEFAULT_NONE_STR.to_string(), |count| count.to_string())
}

fn format_metric(metric: CoverageMetric) -> String {
    format!(
        "{}/{} ({})",
        metric.covered,
        metric.count,
        format_percent(metric.percent)
    )
}

fn format_percent(percent: f64) -> String {
    format!("{percent:.2}%")
}

fn format_delta(before: CoverageMetric, after: CoverageMetric) -> String {
    let covered_delta = after.covered.cast_signed() - before.covered.cast_signed();
    let count_delta = after.count.cast_signed() - before.count.cast_signed();
    format!(
        "{} / {} ({})",
        format_signed_count(covered_delta),
        format_signed_count(count_delta),
        format_percent_delta(percent_delta(before, after))
    )
}

fn format_signed_count(delta: i64) -> String {
    if delta > 0 {
        format!("+{delta}")
    } else {
        delta.to_string()
    }
}

fn format_percent_delta(delta: f64) -> String {
    if delta > 0.0 {
        format!("+{}", format_percent(delta))
    } else {
        format_percent(delta)
    }
}

fn escape_markdown_code(value: &str) -> String {
    value.replace('`', "\\`")
}
