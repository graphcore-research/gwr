// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Manage source files for coverage reports.
//!
//! In order to perform effective coverage comparisons the source files need to
//! be aligned. The aligner first builds a longest-common-subsequence (LCS)
//! table over whole lines. Exact line matches are paired, while insertions and
//! deletions are emitted as one-sided rows. Those unmatched runs are then
//! refined with a second dynamic-programming pass that pairs lines whose
//! character-level LCS similarity is high enough, allowing small edits on the
//! same logical line to appear side by side instead of as a delete plus insert.
//! When source text is missing the aligner falls back to matching line numbers
//! directly. When the source pair is too large to align and the source differs,
//! no source diff is rendered because line-number matching would be misleading.

use std::collections::BTreeMap;

use crate::coverage::file::{CoverageFile, LineCoverage};
use crate::dp_table::DpTable;
use crate::lcs::longest_common_subsequence_lengths;

#[derive(Debug, Default)]
pub(crate) struct LineChanges {
    pub(crate) rows: Vec<LineDiffRow>,
    pub(crate) unavailable_reason: Option<String>,
}

#[derive(Debug)]
pub(crate) struct LineDiffRow {
    pub(crate) aligned_line: AlignedLine,
    pub(crate) before_count: Option<u64>,
    pub(crate) after_count: Option<u64>,
    pub(crate) marker: char,
}

impl LineDiffRow {
    pub(crate) fn new(
        aligned_line: AlignedLine,
        before_count: Option<u64>,
        after_count: Option<u64>,
    ) -> Self {
        let before_covered = before_count.is_some_and(|count| count > 0);
        let after_covered = after_count.is_some_and(|count| count > 0);
        let marker = match (
            aligned_line.before_line_number,
            aligned_line.after_line_number,
        ) {
            (None, Some(_)) => '+',
            (Some(_), None) => '-',
            _ if !before_covered && after_covered => '+',
            _ if before_covered && !after_covered => '-',
            _ => ' ',
        };

        Self {
            aligned_line,
            before_count,
            after_count,
            marker,
        }
    }

    pub(crate) fn changed(&self) -> bool {
        self.marker == '+' || self.marker == '-'
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AlignedLine {
    pub(crate) before_line_number: Option<u64>,
    pub(crate) after_line_number: Option<u64>,
}

struct Aligner<'a> {
    before_source_lines: &'a [String],
    after_source_lines: &'a [String],
}

impl<'a> Aligner<'a> {
    fn new(before_source_lines: &'a [String], after_source_lines: &'a [String]) -> Self {
        Self {
            before_source_lines,
            after_source_lines,
        }
    }

    fn before_num_lines(&self) -> usize {
        self.before_source_lines.len()
    }

    fn after_num_lines(&self) -> usize {
        self.after_source_lines.len()
    }

    fn align_source_lines(&self) -> Vec<AlignedLine> {
        let before_num_lines = self.before_num_lines();
        let after_num_lines = self.after_num_lines();
        let exact_match_lengths = longest_common_subsequence_lengths(
            before_num_lines,
            after_num_lines,
            |before_index, after_index| {
                self.before_source_lines[before_index] == self.after_source_lines[after_index]
            },
        );

        let mut aligned_lines = Vec::new();
        let mut before_index = 0;
        let mut after_index = 0;
        while before_index < before_num_lines || after_index < after_num_lines {
            if before_index < before_num_lines
                && after_index < after_num_lines
                && self.before_source_lines[before_index] == self.after_source_lines[after_index]
            {
                aligned_lines.push(AlignedLine {
                    before_line_number: Some((before_index + 1) as u64),
                    after_line_number: Some((after_index + 1) as u64),
                });
                before_index += 1;
                after_index += 1;
            } else if after_index == after_num_lines
                || (before_index < before_num_lines
                    && exact_match_lengths.get(before_index + 1, after_index)
                        >= exact_match_lengths.get(before_index, after_index + 1))
            {
                aligned_lines.push(AlignedLine {
                    before_line_number: Some((before_index + 1) as u64),
                    after_line_number: None,
                });
                before_index += 1;
            } else {
                aligned_lines.push(AlignedLine {
                    before_line_number: None,
                    after_line_number: Some((after_index + 1) as u64),
                });
                after_index += 1;
            }
        }

        self.refine_unmatched_sections(&aligned_lines)
    }

    fn refine_unmatched_sections(&self, aligned_lines: &[AlignedLine]) -> Vec<AlignedLine> {
        let mut refined = Vec::with_capacity(aligned_lines.len());
        let mut index = 0;

        while index < aligned_lines.len() {
            if aligned_lines[index].before_line_number.is_some()
                && aligned_lines[index].after_line_number.is_some()
            {
                refined.push(aligned_lines[index]);
                index += 1;
                continue;
            }

            let run_start = index;
            while index < aligned_lines.len()
                && !(aligned_lines[index].before_line_number.is_some()
                    && aligned_lines[index].after_line_number.is_some())
            {
                index += 1;
            }

            refined.extend(self.refine_unmatched_run(&aligned_lines[run_start..index]));
        }

        refined
    }

    fn refine_unmatched_run(&self, run: &[AlignedLine]) -> Vec<AlignedLine> {
        let before_line_numbers = run
            .iter()
            .filter_map(|aligned_line| aligned_line.before_line_number)
            .collect::<Vec<_>>();
        let after_line_numbers = run
            .iter()
            .filter_map(|aligned_line| aligned_line.after_line_number)
            .collect::<Vec<_>>();

        if before_line_numbers.is_empty() || after_line_numbers.is_empty() {
            return run.to_vec();
        }

        self.align_similar_lines(&before_line_numbers, &after_line_numbers)
    }

    fn align_similar_lines(
        &self,
        before_line_numbers: &[u64],
        after_line_numbers: &[u64],
    ) -> Vec<AlignedLine> {
        const MIN_LINE_SIMILARITY: f64 = 0.45;
        const SKIP_PENALTY: f64 = 0.15;

        let before_len = before_line_numbers.len();
        let after_len = after_line_numbers.len();
        let mut scores = DpTable::new(before_len + 1, after_len + 1, 0.0);

        for before_index in (0..before_len).rev() {
            scores.set(
                before_index,
                after_len,
                scores.get(before_index + 1, after_len) - SKIP_PENALTY,
            );
        }
        for after_index in (0..after_len).rev() {
            scores.set(
                before_len,
                after_index,
                scores.get(before_len, after_index + 1) - SKIP_PENALTY,
            );
        }

        for before_index in (0..before_len).rev() {
            for after_index in (0..after_len).rev() {
                let similarity = line_similarity(
                    self.before_source_at(before_line_numbers[before_index]),
                    self.after_source_at(after_line_numbers[after_index]),
                );
                let pair_score = if similarity >= MIN_LINE_SIMILARITY {
                    scores.get(before_index + 1, after_index + 1) + similarity
                } else {
                    f64::NEG_INFINITY
                };
                let skip_before = scores.get(before_index + 1, after_index) - SKIP_PENALTY;
                let skip_after = scores.get(before_index, after_index + 1) - SKIP_PENALTY;
                scores.set(
                    before_index,
                    after_index,
                    pair_score.max(skip_before).max(skip_after),
                );
            }
        }

        let mut aligned_lines = Vec::with_capacity(before_len.max(after_len));
        let mut before_index = 0;
        let mut after_index = 0;
        while before_index < before_len || after_index < after_len {
            if before_index == before_len {
                aligned_lines.push(AlignedLine {
                    before_line_number: None,
                    after_line_number: Some(after_line_numbers[after_index]),
                });
                after_index += 1;
                continue;
            }
            if after_index == after_len {
                aligned_lines.push(AlignedLine {
                    before_line_number: Some(before_line_numbers[before_index]),
                    after_line_number: None,
                });
                before_index += 1;
                continue;
            }

            let similarity = line_similarity(
                self.before_source_at(before_line_numbers[before_index]),
                self.after_source_at(after_line_numbers[after_index]),
            );
            let pair_score = if similarity >= MIN_LINE_SIMILARITY {
                scores.get(before_index + 1, after_index + 1) + similarity
            } else {
                f64::NEG_INFINITY
            };
            let skip_before = scores.get(before_index + 1, after_index) - SKIP_PENALTY;
            let skip_after = scores.get(before_index, after_index + 1) - SKIP_PENALTY;

            if pair_score >= skip_before && pair_score >= skip_after {
                aligned_lines.push(AlignedLine {
                    before_line_number: Some(before_line_numbers[before_index]),
                    after_line_number: Some(after_line_numbers[after_index]),
                });
                before_index += 1;
                after_index += 1;
            } else if skip_before >= skip_after {
                aligned_lines.push(AlignedLine {
                    before_line_number: Some(before_line_numbers[before_index]),
                    after_line_number: None,
                });
                before_index += 1;
            } else {
                aligned_lines.push(AlignedLine {
                    before_line_number: None,
                    after_line_number: Some(after_line_numbers[after_index]),
                });
                after_index += 1;
            }
        }

        aligned_lines
    }

    fn before_source_at(&self, line_number: u64) -> &str {
        source_at(self.before_source_lines, line_number)
    }

    fn after_source_at(&self, line_number: u64) -> &str {
        source_at(self.after_source_lines, line_number)
    }
}

#[derive(Debug)]
enum SourceAlignment {
    Aligned(Vec<AlignedLine>),
    Unavailable(String),
}

pub(crate) fn line_changes(
    before: Option<&CoverageFile>,
    after: Option<&CoverageFile>,
    before_source_lines: Option<&[String]>,
    after_source_lines: Option<&[String]>,
) -> LineChanges {
    let before_coverage = before.map(CoverageFile::line_coverage).unwrap_or_default();
    let after_coverage = after.map(CoverageFile::line_coverage).unwrap_or_default();
    let max_line_number = max_line_number(
        &before_coverage,
        &after_coverage,
        before_source_lines,
        after_source_lines,
    );
    let aligned_lines = match (before, after) {
        (None, Some(_)) => SourceAlignment::Aligned(align_added_lines(max_line_number)),
        (Some(_), None) => SourceAlignment::Aligned(align_removed_lines(max_line_number)),
        _ => try_to_align_lines(before_source_lines, after_source_lines, max_line_number),
    };
    let has_source_alignment = before_source_lines.is_some() || after_source_lines.is_some();

    let mut changes = LineChanges::default();
    let aligned_lines = match aligned_lines {
        SourceAlignment::Aligned(aligned_lines) => aligned_lines,
        SourceAlignment::Unavailable(reason) => {
            changes.unavailable_reason = Some(reason);
            return changes;
        }
    };

    for aligned_line in aligned_lines {
        let before_count = aligned_line
            .before_line_number
            .and_then(|line_number| before_coverage.get(&line_number))
            .map(|coverage| coverage.count);
        let after_count = aligned_line
            .after_line_number
            .and_then(|line_number| after_coverage.get(&line_number))
            .map(|coverage| coverage.count);

        if has_source_alignment || before_count.is_some() || after_count.is_some() {
            changes
                .rows
                .push(LineDiffRow::new(aligned_line, before_count, after_count));
        }
    }

    changes
}

fn max_line_number(
    before_coverage: &BTreeMap<u64, LineCoverage>,
    after_coverage: &BTreeMap<u64, LineCoverage>,
    before_source_lines: Option<&[String]>,
    after_source_lines: Option<&[String]>,
) -> u64 {
    let max_covered_line = before_coverage
        .keys()
        .chain(after_coverage.keys())
        .copied()
        .max()
        .unwrap_or(0);

    max_covered_line
        .max(before_source_lines.map_or(0, |lines| lines.len() as u64))
        .max(after_source_lines.map_or(0, |lines| lines.len() as u64))
}

fn align_added_lines(max_line_number: u64) -> Vec<AlignedLine> {
    (1..=max_line_number)
        .map(|line_number| AlignedLine {
            before_line_number: None,
            after_line_number: Some(line_number),
        })
        .collect()
}

fn align_removed_lines(max_line_number: u64) -> Vec<AlignedLine> {
    (1..=max_line_number)
        .map(|line_number| AlignedLine {
            before_line_number: Some(line_number),
            after_line_number: None,
        })
        .collect()
}

#[derive(Debug)]
pub(crate) struct LineChangeHunk {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl LineChanges {
    pub(crate) fn has_changes(&self) -> bool {
        self.rows.iter().any(LineDiffRow::changed)
    }

    pub(crate) fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable_reason.as_deref()
    }

    pub(crate) fn changed_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row.changed().then_some(index))
            .collect()
    }
}

pub(crate) fn line_change_hunks(
    changed_rows: &[usize],
    context: usize,
    row_count: usize,
) -> Vec<LineChangeHunk> {
    let mut hunks: Vec<LineChangeHunk> = Vec::new();
    for row in changed_rows {
        let start = row.saturating_sub(context);
        let end = row.saturating_add(context + 1).min(row_count);

        match hunks.last_mut() {
            Some(hunk) if start <= hunk.end => {
                hunk.end = hunk.end.max(end);
            }
            _ => hunks.push(LineChangeHunk { start, end }),
        }
    }

    hunks
}

fn try_to_align_lines(
    before_source_lines: Option<&[String]>,
    after_source_lines: Option<&[String]>,
    max_covered_line: u64,
) -> SourceAlignment {
    match (before_source_lines, after_source_lines) {
        (Some(before_source_lines), Some(after_source_lines))
            if !too_many_lines_to_align(before_source_lines, after_source_lines) =>
        {
            SourceAlignment::Aligned(
                Aligner::new(before_source_lines, after_source_lines).align_source_lines(),
            )
        }
        (Some(before_source_lines), Some(after_source_lines))
            if before_source_lines != after_source_lines =>
        {
            SourceAlignment::Unavailable(format!(
                "source files differ, but they are too large to align safely ({} x {} lines exceeds the {} line-pair limit)",
                before_source_lines.len(),
                after_source_lines.len(),
                MAX_LINE_PERMUTATIONS,
            ))
        }
        _ => SourceAlignment::Aligned(
            (1..=max_covered_line)
                .map(|line_number| AlignedLine {
                    before_line_number: Some(line_number),
                    after_line_number: Some(line_number),
                })
                .collect(),
        ),
    }
}

const MAX_LINE_PERMUTATIONS: usize = 8_000_000;

fn too_many_lines_to_align(before_source_lines: &[String], after_source_lines: &[String]) -> bool {
    before_source_lines
        .len()
        .saturating_mul(after_source_lines.len())
        > MAX_LINE_PERMUTATIONS
}

fn source_at(source_lines: &[String], line_number: u64) -> &str {
    source_lines
        .get(line_number.saturating_sub(1) as usize)
        .map_or("", String::as_str)
}

fn line_similarity(before: &str, after: &str) -> f64 {
    if before == after {
        return 1.0;
    }

    let before = before.trim();
    let after = after.trim();
    if before.is_empty() || after.is_empty() {
        return 0.0;
    }

    let before_chars = before.chars().collect::<Vec<_>>();
    let after_chars = after.chars().collect::<Vec<_>>();
    let lengths = longest_common_subsequence_lengths(
        before_chars.len(),
        after_chars.len(),
        |before_index, after_index| before_chars[before_index] == after_chars[after_index],
    );

    (lengths.get(0, 0) * 2) as f64 / (before_chars.len() + after_chars.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::{
        AlignedLine, MAX_LINE_PERMUTATIONS, SourceAlignment, line_changes, line_similarity,
        try_to_align_lines,
    };

    fn source(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    fn aligned(before_line_number: Option<u64>, after_line_number: Option<u64>) -> AlignedLine {
        AlignedLine {
            before_line_number,
            after_line_number,
        }
    }

    fn aligned_source_lines(
        before_source_lines: Option<&[String]>,
        after_source_lines: Option<&[String]>,
        max_covered_line: u64,
    ) -> Vec<AlignedLine> {
        match try_to_align_lines(before_source_lines, after_source_lines, max_covered_line) {
            SourceAlignment::Aligned(lines) => lines,
            SourceAlignment::Unavailable(reason) => {
                panic!("expected aligned source lines, got unavailable: {reason}")
            }
        }
    }

    #[test]
    fn aligns_inserted_source_lines_between_exact_matches() {
        let before = source(&["fn first() {}", "fn target() {}"]);
        let after = source(&["fn first() {}", "fn inserted() {}", "fn target() {}"]);

        assert_eq!(
            aligned_source_lines(Some(&before), Some(&after), 3),
            vec![
                aligned(Some(1), Some(1)),
                aligned(None, Some(2)),
                aligned(Some(2), Some(3))
            ]
        );
    }

    #[test]
    fn pairs_similar_edited_lines_inside_unmatched_runs() {
        let before = source(&["fn first() {}", "fn target_old_value() {}", "fn last() {}"]);
        let after = source(&[
            "fn first() {}",
            "fn inserted() {}",
            "fn target_new_value() {}",
            "fn last() {}",
        ]);

        assert_eq!(
            aligned_source_lines(Some(&before), Some(&after), 4),
            vec![
                aligned(Some(1), Some(1)),
                aligned(None, Some(2)),
                aligned(Some(2), Some(3)),
                aligned(Some(3), Some(4)),
            ]
        );
    }

    #[test]
    fn falls_back_to_matching_line_numbers_without_both_sources() {
        let before = source(&["before one", "before two"]);

        assert_eq!(
            aligned_source_lines(Some(&before), None, 3),
            vec![
                aligned(Some(1), Some(1)),
                aligned(Some(2), Some(2)),
                aligned(Some(3), Some(3)),
            ]
        );
    }

    #[test]
    fn aligns_deleted_source_lines_between_exact_matches() {
        let before = source(&["fn first() {}", "fn deleted() {}", "fn target() {}"]);
        let after = source(&["fn first() {}", "fn target() {}"]);

        assert_eq!(
            aligned_source_lines(Some(&before), Some(&after), 3),
            vec![
                aligned(Some(1), Some(1)),
                aligned(Some(2), None),
                aligned(Some(3), Some(2)),
            ]
        );
    }

    #[test]
    fn leaves_dissimilar_unmatched_lines_unpaired() {
        let before = source(&["fn first() {}", "alpha alpha alpha", "fn last() {}"]);
        let after = source(&["fn first() {}", "z9 z9 z9", "fn last() {}"]);

        assert_eq!(
            aligned_source_lines(Some(&before), Some(&after), 3),
            vec![
                aligned(Some(1), Some(1)),
                aligned(Some(2), None),
                aligned(None, Some(2)),
                aligned(Some(3), Some(3)),
            ]
        );
    }

    #[test]
    fn keeps_trailing_after_lines_after_similar_pair_in_unmatched_run() {
        let before = source(&["fn first() {}", "fn target_old_value() {}", "fn last() {}"]);
        let after = source(&[
            "fn first() {}",
            "fn target_new_value() {}",
            "fn after_tail() {}",
            "fn last() {}",
        ]);

        assert_eq!(
            aligned_source_lines(Some(&before), Some(&after), 4),
            vec![
                aligned(Some(1), Some(1)),
                aligned(Some(2), Some(2)),
                aligned(None, Some(3)),
                aligned(Some(3), Some(4)),
            ]
        );
    }

    #[test]
    fn keeps_trailing_before_lines_after_similar_pair_in_unmatched_run() {
        let before = source(&[
            "fn first() {}",
            "fn target_old_value() {}",
            "fn before_tail() {}",
            "fn last() {}",
        ]);
        let after = source(&["fn first() {}", "fn target_new_value() {}", "fn last() {}"]);

        assert_eq!(
            aligned_source_lines(Some(&before), Some(&after), 4),
            vec![
                aligned(Some(1), Some(1)),
                aligned(Some(2), Some(2)),
                aligned(Some(3), None),
                aligned(Some(4), Some(3)),
            ]
        );
    }

    #[test]
    fn falls_back_to_matching_line_numbers_when_identical_sources_are_too_large() {
        let before = vec!["same".to_string(); 2_829];
        let after = before.clone();

        assert_eq!(
            aligned_source_lines(Some(&before), Some(&after), 3),
            vec![
                aligned(Some(1), Some(1)),
                aligned(Some(2), Some(2)),
                aligned(Some(3), Some(3)),
            ]
        );
    }

    #[test]
    fn reports_unavailable_diff_when_changed_sources_are_too_large_to_align() {
        let before = vec!["before".to_string(); 2_829];
        let after = vec!["after".to_string(); 2_829];

        let SourceAlignment::Unavailable(reason) =
            try_to_align_lines(Some(&before), Some(&after), 3)
        else {
            panic!("expected source alignment to be unavailable");
        };
        assert!(reason.contains("source files differ"));
        assert!(
            reason
                .contains(format!("exceeds the {MAX_LINE_PERMUTATIONS} line-pair limit").as_str())
        );

        let changes = line_changes(None, None, Some(&before), Some(&after));
        assert_eq!(changes.unavailable_reason(), Some(reason.as_str()));
    }

    #[test]
    fn line_similarity_handles_exact_empty_and_partial_matches() {
        assert_eq!(line_similarity("same", "same"), 1.0);
        assert_eq!(line_similarity("   ", "same"), 0.0);
        assert_eq!(line_similarity("abc", "axc"), 2.0 / 3.0);
    }
}
