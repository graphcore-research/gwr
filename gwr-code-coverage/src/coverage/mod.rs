// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use crate::coverage::file::CoverageFile;

pub mod file;
pub mod report;

fn strip_prefix(filename: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return filename.to_string();
    }

    filename.strip_prefix(prefix).map_or_else(
        || filename.to_string(),
        |filename| filename.trim_start_matches('/').to_string(),
    )
}

#[must_use]
pub(crate) fn line_coverage_summary_delta(
    before_file: Option<&CoverageFile>,
    after_file: Option<&CoverageFile>,
) -> Option<i64> {
    let before_covered = before_file.map_or(0, |file| file.summary.lines.covered.cast_signed());
    let after_covered = after_file.map_or(0, |file| file.summary.lines.covered.cast_signed());
    let delta = after_covered - before_covered;
    (delta != 0).then_some(delta)
}
