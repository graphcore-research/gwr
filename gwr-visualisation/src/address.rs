// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AddressRange {
    pub(crate) start: u128,
    pub(crate) end: u128,
}

impl AddressRange {
    pub(crate) fn new(start: u64, bytes: u64) -> Self {
        let start = u128::from(start);
        Self::from_bounds(start, start + u128::from(bytes))
    }

    pub(crate) fn from_bounds(start: u128, end: u128) -> Self {
        debug_assert!(end >= start);
        Self { start, end }
    }

    pub(crate) fn intersection(self, other: Self) -> Option<Self> {
        let intersection = Self {
            start: self.start.max(other.start),
            end: self.end.min(other.end),
        };
        (intersection.end > intersection.start).then_some(intersection)
    }

    pub(crate) fn len(self) -> u128 {
        self.end - self.start
    }
}

pub(crate) fn range_union_length(ranges: impl IntoIterator<Item = AddressRange>) -> u128 {
    merge_ranges(ranges)
        .into_iter()
        .map(AddressRange::len)
        .sum()
}

pub(crate) fn merge_ranges(ranges: impl IntoIterator<Item = AddressRange>) -> Vec<AddressRange> {
    let mut ranges = ranges
        .into_iter()
        .filter(|range| range.end > range.start)
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<AddressRange> = Vec::new();
    for range in ranges {
        if let Some(current) = merged.last_mut()
            && range.start <= current.end
        {
            current.end = current.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::{AddressRange, range_union_length};

    #[test]
    fn unions_overlapping_address_ranges_once() {
        assert_eq!(
            range_union_length([
                AddressRange::new(0, 8),
                AddressRange::new(4, 8),
                AddressRange::new(16, 4),
            ]),
            16
        );
    }
}
