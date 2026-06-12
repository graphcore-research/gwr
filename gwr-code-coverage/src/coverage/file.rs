// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::{fmt, fs};

use serde::de::{Deserializer, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, de};

use crate::CoverageReport;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CoverageFile {
    pub(crate) filename: String,
    pub(crate) summary: CoverageSummary,
    #[serde(default)]
    segments: Vec<CoverageSegment>,
}

impl CoverageFile {
    pub(crate) fn has_line_details(&self) -> bool {
        !self.segments.is_empty()
    }

    pub(crate) fn source_lines(&self) -> Option<Vec<String>> {
        fs::read_to_string(&self.filename)
            .ok()
            .map(|source| source.lines().map(ToString::to_string).collect())
    }

    pub(crate) fn line_coverage(&self) -> BTreeMap<u64, LineCoverage> {
        let mut lines = BTreeMap::new();
        for (index, segment) in self.segments.iter().enumerate() {
            if !segment.has_count {
                continue;
            }

            let end_line = self
                .segments
                .iter()
                .skip(index + 1)
                .find(|next_segment| {
                    next_segment.line > segment.line
                        || (next_segment.line == segment.line
                            && next_segment.column > segment.column)
                })
                .map_or(segment.line, |next_segment| {
                    if next_segment.line == segment.line {
                        segment.line
                    } else {
                        next_segment.line - 1
                    }
                });

            for line in segment.line..=end_line {
                lines
                    .entry(line)
                    .and_modify(|line: &mut LineCoverage| {
                        line.count = line.count.max(segment.count);
                    })
                    .or_insert(LineCoverage {
                        count: segment.count,
                    });
            }
        }

        lines
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LineCoverage {
    pub(crate) count: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CoverageSummary {
    pub(crate) lines: CoverageMetric,
    pub(crate) functions: CoverageMetric,
    pub(crate) regions: CoverageMetric,
}

impl CoverageSummary {
    pub(crate) fn lines(&self) -> CoverageMetric {
        self.lines
    }

    pub(crate) fn functions(&self) -> CoverageMetric {
        self.functions
    }

    pub(crate) fn regions(&self) -> CoverageMetric {
        self.regions
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct CoverageMetric {
    pub(crate) count: u64,
    pub(crate) covered: u64,
    pub(crate) percent: f64,
}

impl PartialEq for CoverageMetric {
    fn eq(&self, other: &Self) -> bool {
        self.covered == other.covered
            && self.count == other.count
            && percent_delta(*self, *other).abs() < f64::EPSILON
    }
}

impl Eq for CoverageMetric {}

#[must_use]
pub fn coverage_did_not_decrease(before: &CoverageReport, after: &CoverageReport) -> bool {
    let before = before.totals();
    let after = after.totals();

    percent_delta(before.lines, after.lines) >= 0.0
        && percent_delta(before.functions, after.functions) >= 0.0
        && percent_delta(before.regions, after.regions) >= 0.0
}

#[must_use]
pub(crate) fn percent_delta(before: CoverageMetric, after: CoverageMetric) -> f64 {
    after.percent - before.percent
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CoverageSegment {
    pub(crate) line: u64,
    pub(crate) column: u64,
    pub(crate) count: u64,
    pub(crate) has_count: bool,
}

impl<'de> Deserialize<'de> for CoverageSegment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(CoverageSegmentVisitor)
    }
}

struct CoverageSegmentVisitor;

impl<'de> Visitor<'de> for CoverageSegmentVisitor {
    type Value = CoverageSegment;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an llvm-cov segment array")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let line = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;
        let column = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(1, &self))?;
        let count = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(2, &self))?;
        let has_count = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(3, &self))?;
        while seq.next_element::<IgnoredAny>()?.is_some() {}

        Ok(CoverageSegment {
            line,
            column,
            count,
            has_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CoverageSegment;

    fn segment_error(json: &str) -> String {
        serde_json::from_str::<CoverageSegment>(json)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn segment_reports_error_when_line_is_missing() {
        assert!(segment_error("[]").contains("invalid length 0"));
    }

    #[test]
    fn segment_reports_error_when_column_is_missing() {
        assert!(segment_error("[1]").contains("invalid length 1"));
    }

    #[test]
    fn segment_reports_error_when_count_is_missing() {
        assert!(segment_error("[1,2]").contains("invalid length 2"));
    }

    #[test]
    fn segment_reports_error_when_has_count_is_missing() {
        assert!(segment_error("[1,2,3]").contains("invalid length 3"));
    }

    #[test]
    fn segment_reports_expected_type_for_non_array_input() {
        assert!(segment_error("42").contains("an llvm-cov segment array"));
    }

    #[test]
    fn segment_ignores_extra_fields() {
        let segment: CoverageSegment =
            serde_json::from_str("[1,2,3,true,false,\"ignored\"]").unwrap();

        assert_eq!(segment.line, 1);
        assert_eq!(segment.column, 2);
        assert_eq!(segment.count, 3);
        assert!(segment.has_count);
    }
}
