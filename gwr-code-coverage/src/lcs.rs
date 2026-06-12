// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Longest-common-subsequence table construction.
//!
//! This module owns the shared dynamic-programming core. Callers provide the
//! left/right sequence lengths and a comparison function over indices, and the
//! returned table stores the best common-subsequence length for every pair of
//! suffixes.

use crate::dp_table::DpTable;

pub(crate) fn longest_common_subsequence_lengths(
    left_len: usize,
    right_len: usize,
    matches: impl Fn(usize, usize) -> bool,
) -> DpTable<usize> {
    let mut lengths = DpTable::new(left_len + 1, right_len + 1, 0usize);

    for left_index in (0..left_len).rev() {
        for right_index in (0..right_len).rev() {
            let length = if matches(left_index, right_index) {
                lengths.get(left_index + 1, right_index + 1) + 1
            } else {
                lengths
                    .get(left_index + 1, right_index)
                    .max(lengths.get(left_index, right_index + 1))
            };
            lengths.set(left_index, right_index, length);
        }
    }

    lengths
}

#[cfg(test)]
mod tests {
    use super::longest_common_subsequence_lengths;
    use crate::dp_table::DpTable;

    struct LcsFixture<'a> {
        left: &'a [&'a str],
        right: &'a [&'a str],
    }

    impl<'a> LcsFixture<'a> {
        fn new(left: &'a [&'a str], right: &'a [&'a str]) -> Self {
            Self { left, right }
        }

        fn lengths(&self) -> DpTable<usize> {
            longest_common_subsequence_lengths(self.left.len(), self.right.len(), |left, right| {
                self.matches(left, right)
            })
        }

        fn matches(&self, left_index: usize, right_index: usize) -> bool {
            assert!(!self.left.is_empty());
            assert!(!self.right.is_empty());
            self.left[left_index] == self.right[right_index]
        }
    }

    #[test]
    fn counts_exact_matches() {
        let lengths = LcsFixture::new(&["a", "b", "c"], &["a", "b", "c"]).lengths();

        assert_eq!(lengths.get(0, 0), 3);
        assert_eq!(lengths.get(1, 1), 2);
        assert_eq!(lengths.get(2, 2), 1);
    }

    #[test]
    fn skips_unmatched_items() {
        let lengths = LcsFixture::new(&["a", "x", "b", "c"], &["a", "b", "y", "c"]).lengths();

        assert_eq!(lengths.get(0, 0), 3);
        assert_eq!(lengths.get(1, 1), 2);
        assert_eq!(lengths.get(2, 2), 1);
    }

    #[test]
    fn handles_empty_sides() {
        let lengths = LcsFixture::new(&[], &["a", "b", "c"]).lengths();

        assert_eq!(lengths.get(0, 0), 0);
        assert_eq!(lengths.get(0, 3), 0);
    }
}
