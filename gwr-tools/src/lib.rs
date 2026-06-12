// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;
use std::{fmt, fs};

use serde::Deserialize;

const FILE_METRICS: [&str; 4] = ["Combined", "Lines", "Functions", "Regions"];

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    EmptyReport,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(error) => write!(f, "failed to read coverage report: {error}"),
            Error::Json(error) => write!(f, "failed to parse coverage report: {error}"),
            Error::EmptyReport => write!(f, "coverage report did not contain any data"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Json(error)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoverageReport {
    data: Vec<CoverageData>,
}

impl CoverageReport {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, Error> {
        let report = fs::read_to_string(path)?;
        Self::from_json(&report)
    }

    pub fn from_json(report: &str) -> Result<Self, Error> {
        let report: Self = serde_json::from_str(report)?;
        if report.data.is_empty() {
            return Err(Error::EmptyReport);
        }
        Ok(report)
    }

    fn totals(&self) -> &CoverageSummary {
        &self.data[0].totals
    }

    fn files(&self, prefix: &str) -> BTreeMap<String, &CoverageSummary> {
        self.data[0]
            .files
            .iter()
            .map(|file| (strip_prefix(&file.filename, prefix), &file.summary))
            .collect()
    }

    fn filenames(&self) -> impl Iterator<Item = &str> {
        self.data[0].files.iter().map(|file| file.filename.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct CoverageData {
    files: Vec<CoverageFile>,
    totals: CoverageSummary,
}

#[derive(Debug, Clone, Deserialize)]
struct CoverageFile {
    filename: String,
    summary: CoverageSummary,
}

#[derive(Debug, Clone, Deserialize)]
struct CoverageSummary {
    lines: CoverageMetric,
    functions: CoverageMetric,
    regions: CoverageMetric,
}

impl CoverageSummary {
    fn combined(&self) -> CoverageMetric {
        let covered = self.lines.covered + self.functions.covered + self.regions.covered;
        let count = self.lines.count + self.functions.count + self.regions.count;

        CoverageMetric {
            count,
            covered,
            percent: percent(covered, count),
        }
    }

    fn lines(&self) -> CoverageMetric {
        self.lines
    }

    fn functions(&self) -> CoverageMetric {
        self.functions
    }

    fn regions(&self) -> CoverageMetric {
        self.regions
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct CoverageMetric {
    count: u64,
    covered: u64,
    percent: f64,
}

#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    pub show_all_files: bool,
    pub before_path: Option<String>,
    pub after_path: Option<String>,
    pub before_prefix: Option<String>,
    pub after_prefix: Option<String>,
}

#[must_use]
pub fn render_markdown(
    before: &CoverageReport,
    after: &CoverageReport,
    show_all_files: bool,
) -> String {
    render_markdown_with_options(
        before,
        after,
        &RenderOptions {
            show_all_files,
            ..RenderOptions::default()
        },
    )
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

    report
}

#[must_use]
pub fn combined_total_coverage_delta(before: &CoverageReport, after: &CoverageReport) -> f64 {
    percent_delta(before.totals().combined(), after.totals().combined())
}

#[must_use]
pub fn combined_total_coverage_is_non_negative(
    before: &CoverageReport,
    after: &CoverageReport,
) -> bool {
    combined_total_coverage_delta(before, after) >= 0.0
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

fn render_totals_section(before: &CoverageSummary, after: &CoverageSummary) -> String {
    let mut section = String::new();
    section.push_str("## Totals\n\n");
    section.push_str("| Metric | Before | After | Delta |\n");
    section.push_str("| --- | ---: | ---: | ---: |\n");
    push_metric_row(
        &mut section,
        "Combined",
        before.combined(),
        after.combined(),
    );
    push_metric_row(&mut section, "Lines", before.lines, after.lines);
    push_metric_row(&mut section, "Functions", before.functions, after.functions);
    push_metric_row(&mut section, "Regions", before.regions, after.regions);
    section
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
            .is_some_and(|(before, after)| summaries_equal(before, after));

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
    for metric in FILE_METRICS {
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

fn push_coverage_file_row(report: &mut String, label: &str, path: Option<&str>, prefix: &str) {
    let path = path.map_or("n/a".to_string(), format_code_value);
    let prefix = if prefix.is_empty() {
        "none".to_string()
    } else {
        format_code_value(prefix)
    };
    let _ = writeln!(report, "| {label} | {path} | {prefix} |");
}

fn format_code_value(value: &str) -> String {
    format!("`{}`", escape_markdown_code(value))
}

fn infer_prefix<'a>(filenames: impl Iterator<Item = &'a str>) -> String {
    let mut common: Option<String> = None;
    for filename in filenames {
        if !filename.starts_with('/') {
            return String::new();
        }

        let dirname = filename.rsplit_once('/').map_or("", |(dirname, _)| dirname);
        common = Some(match common {
            Some(common) => common_directory_prefix(common.as_str(), dirname),
            None => dirname.to_string(),
        });
    }

    common.as_deref().map_or_else(String::new, normalize_prefix)
}

fn common_directory_prefix(left: &str, right: &str) -> String {
    left.split('/')
        .zip(right.split('/'))
        .take_while(|(left, right)| left == right)
        .map(|(component, _)| component)
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_prefix(prefix: &str) -> String {
    prefix.trim_end_matches('/').to_string()
}

fn strip_prefix(filename: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return filename.to_string();
    }

    filename.strip_prefix(prefix).map_or_else(
        || filename.to_string(),
        |filename| filename.trim_start_matches('/').to_string(),
    )
}

fn summaries_equal(before: &CoverageSummary, after: &CoverageSummary) -> bool {
    metrics_equal(before.lines, after.lines)
        && metrics_equal(before.functions, after.functions)
        && metrics_equal(before.regions, after.regions)
}

fn metrics_equal(before: CoverageMetric, after: CoverageMetric) -> bool {
    before.covered == after.covered
        && before.count == after.count
        && percent_delta(before, after).abs() < f64::EPSILON
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

fn push_file_row(
    report: &mut String,
    filename: &str,
    before: Option<&CoverageSummary>,
    after: Option<&CoverageSummary>,
) {
    let _ = writeln!(
        report,
        "| `{}` | {} | {} | {} | {} |",
        escape_markdown_code(filename),
        format_file_metric_group(before, after, CoverageSummary::combined),
        format_file_metric_group(before, after, CoverageSummary::lines),
        format_file_metric_group(before, after, CoverageSummary::functions),
        format_file_metric_group(before, after, CoverageSummary::regions),
    );
}

fn format_file_metric_group(
    before: Option<&CoverageSummary>,
    after: Option<&CoverageSummary>,
    metric: fn(&CoverageSummary) -> CoverageMetric,
) -> String {
    let before_metric = before.map(metric);
    let after_metric = after.map(metric);

    format!(
        "{} | {} | {}",
        format_optional_metric(before_metric),
        format_optional_metric(after_metric),
        format_optional_delta(before_metric, after_metric),
    )
}

fn format_optional_metric(metric: Option<CoverageMetric>) -> String {
    metric.map_or("n/a".to_string(), format_metric)
}

fn format_optional_delta(before: Option<CoverageMetric>, after: Option<CoverageMetric>) -> String {
    match (before, after) {
        (Some(before), Some(after)) => format_delta(before, after),
        (None, Some(_)) | (Some(_), None) => "n/a".to_string(),
        (None, None) => unreachable!("file row has at least one metric"),
    }
}

fn format_metric(metric: CoverageMetric) -> String {
    format!(
        "{}/{} ({})",
        metric.covered,
        metric.count,
        format_percent(metric.percent)
    )
}

fn percent_delta(before: CoverageMetric, after: CoverageMetric) -> f64 {
    after.percent - before.percent
}

fn percent(covered: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        (covered as f64 / count as f64) * 100.0
    }
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
