// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Count repeated, disjoint byte ranges by visiting only map-boundary blocks.

use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::types::SimError;

const MAX_FRAGMENTED_RANGES: u128 = 100_000;

use super::{MemoryRegion, TensorView, split_range};

struct Repetition {
    count: u128,
    stride: u128,
}

struct Layout {
    start: u128,
    length: u128,
    repetitions: Vec<Repetition>,
    /// Suffix bounding span and number of leaf ranges at each depth.
    spans: Vec<u128>,
    counts: Vec<u128>,
}

impl Layout {
    fn new(view: &TensorView) -> Result<Self, SimError> {
        let layout = view.layout();
        let bounds = view.address_bounds();
        let (start, length, repetitions) = if bounds.end - bounds.start
            == layout.num_access_bytes() as u128
        {
            // This also handles packed ranges that coalesce to dense coverage.
            (bounds.start, bounds.end - bounds.start, vec![])
        } else if layout.bits_per_element().is_multiple_of(8) {
            let bytes = (layout.bits_per_element() / 8) as u128;
            (
                u128::from(view.tensor().addr()) + layout.first_element() as u128 * bytes,
                layout.elements_per_range() as u128 * bytes,
                layout
                    .strides()
                    .iter()
                    .map(|s| Repetition {
                        count: s.count() as u128,
                        stride: s.stride_elements() as u128 * bytes,
                    })
                    .collect(),
            )
        } else {
            return Err(SimError("Analytical traffic does not yet support fragmented packed views; use explicit exact planning".into()));
        };
        let rank = repetitions.len();
        let mut spans = vec![length; rank + 1];
        let mut counts = vec![1; rank + 1];
        // Tensor validation bounds these products and addresses by the tensor
        // extent; u128 also represents the exclusive end of the address space.
        for depth in (0..rank).rev() {
            spans[depth] =
                spans[depth + 1] + (repetitions[depth].count - 1) * repetitions[depth].stride;
            counts[depth] = counts[depth + 1] * repetitions[depth].count;
        }
        Ok(Self {
            start,
            length,
            repetitions,
            spans,
            counts,
        })
    }

    fn count_in(
        &self,
        depth: usize,
        start: u128,
        selected: &Range<u128>,
        maximum: u128,
        counts: &mut BTreeMap<u128, u128>,
    ) -> Result<(), SimError> {
        let end = start + self.spans[depth];
        if selected.start >= end || selected.end <= start {
            return Ok(());
        }
        if selected.start <= start && end <= selected.end {
            return add_ranges(counts, self.length, self.counts[depth], maximum);
        }
        let Some(repetition) = self.repetitions.get(depth) else {
            return add_ranges(
                counts,
                end.min(selected.end) - start.max(selected.start),
                1,
                maximum,
            );
        };
        let child_span = self.spans[depth + 1];
        let first = if selected.start >= start + child_span {
            (selected.start - start - child_span) / repetition.stride + 1
        } else {
            0
        };
        let limit = ((selected.end - start - 1) / repetition.stride + 1).min(repetition.count);
        if first >= limit {
            return Ok(());
        }
        self.count_in(
            depth + 1,
            start + first * repetition.stride,
            selected,
            maximum,
            counts,
        )?;
        if limit - first == 1 {
            return Ok(());
        }
        self.count_in(
            depth + 1,
            start + (limit - 1) * repetition.stride,
            selected,
            maximum,
            counts,
        )?;
        let middle = limit - first - 2;
        if middle > 0 {
            add_ranges(
                counts,
                self.length,
                middle * self.counts[depth + 1],
                maximum,
            )?;
        }
        Ok(())
    }
}

fn fragmented_packed_range_count(view: &TensorView) -> Result<Option<u128>, SimError> {
    let layout = view.layout();
    let bounds = view.address_bounds();
    if layout.bits_per_element().is_multiple_of(8)
        || bounds.end - bounds.start == layout.num_access_bytes() as u128
    {
        return Ok(None);
    }
    let count = layout.strides().iter().try_fold(1u128, |count, stride| {
        count.checked_mul(stride.count() as u128)
    });
    count
        .map(Some)
        .ok_or_else(|| SimError("Analytical fragmented range count overflow".into()))
}

fn check_fragmented_range_limit(count: u128) -> Result<(), SimError> {
    if count > MAX_FRAGMENTED_RANGES {
        return Err(SimError(format!(
            "Analytical traffic supports at most {MAX_FRAGMENTED_RANGES} fragmented ranges; refusing to traverse {count}"
        )));
    }
    Ok(())
}

fn add_ranges(
    counts: &mut BTreeMap<u128, u128>,
    length: u128,
    repetitions: u128,
    maximum: u128,
) -> Result<(), SimError> {
    for run in split_range(length, maximum, 0)
        .map_err(|e| SimError(format!("Analytical traffic shaping failed: {e:?}")))?
    {
        let count = counts.entry(run.payload_bytes).or_default();
        *count = run
            .count
            .checked_mul(repetitions)
            .and_then(|n| count.checked_add(n))
            .ok_or_else(|| SimError("Analytical transaction count overflow".into()))?;
    }
    Ok(())
}

pub(super) fn summarize(
    view: &TensorView,
    regions: &[MemoryRegion],
    maximum: u128,
) -> Result<BTreeMap<u64, BTreeMap<u128, u128>>, SimError> {
    if let Some(range_count) = fragmented_packed_range_count(view)? {
        check_fragmented_range_limit(range_count)?;
        let mut destinations = BTreeMap::new();
        let mut mapped = 0u128;
        for range in view.address_ranges() {
            for region in regions {
                let start = range.start.max(u128::from(region.start));
                let end = range.end.min(u128::from(region.end) + 1);
                if start >= end {
                    continue;
                }
                mapped = mapped
                    .checked_add(end - start)
                    .ok_or_else(|| SimError("Analytical mapped payload overflow".into()))?;
                add_ranges(
                    destinations.entry(region.device.0).or_default(),
                    end - start,
                    1,
                    maximum,
                )?;
            }
        }
        if mapped != view.layout().num_access_bytes() as u128 {
            return Err(SimError(
                "Unmapped physical bytes in analytical roofline footprint".into(),
            ));
        }
        return Ok(destinations);
    }
    let layout = Layout::new(view)?;
    let mut destinations = BTreeMap::new();
    for region in regions {
        let counts = destinations
            .entry(region.device.0)
            .or_insert_with(BTreeMap::new);
        layout.count_in(
            0,
            layout.start,
            &(u128::from(region.start)..u128::from(region.end) + 1),
            maximum,
            counts,
        )?;
    }
    destinations.retain(|_, counts| !counts.is_empty());
    let mapped = destinations
        .values()
        .flat_map(|counts| counts.iter())
        .try_fold(0u128, |total, (bytes, count)| {
            bytes
                .checked_mul(*count)
                .and_then(|n| total.checked_add(n))
                .ok_or_else(|| SimError("Analytical mapped payload overflow".into()))
        })?;
    if mapped != view.layout().num_access_bytes() as u128 {
        return Err(SimError(
            "Unmapped physical bytes in analytical roofline footprint".into(),
        ));
    }
    Ok(destinations)
}

/// Dense coverage scales with member count, not the number of stripes. Small
/// fragmented layouts use the same arithmetic once per view range; layouts
/// beyond the explicit bound fail before any fragment iteration.
pub(super) fn summarize_striped(
    view: &TensorView,
    map: &crate::memory::memory_map::MemoryMap,
    maximum: u128,
) -> Result<BTreeMap<u64, BTreeMap<u128, u128>>, SimError> {
    if let Some(range_count) = fragmented_packed_range_count(view)? {
        check_fragmented_range_limit(range_count)?;
    } else {
        let layout = Layout::new(view)?;
        check_fragmented_range_limit(layout.counts[0])?;
    }
    let mut destinations = BTreeMap::new();
    for range in view.address_ranges() {
        let mut mapped = 0u128;
        for (region, striping) in map.logical_windows() {
            let start = range.start.max(u128::from(region.start));
            let end = range.end.min(u128::from(region.end) + 1);
            if start >= end {
                continue;
            }
            mapped += end - start;
            let Some((members, stripe)) = striping else {
                add_ranges(
                    destinations.entry(region.device.0).or_default(),
                    end - start,
                    1,
                    maximum,
                )?;
                continue;
            };
            let stripe = u128::from(stripe);
            let member_count = members.len() as u128;
            let relative_start = start - u128::from(region.start);
            let relative_end = end - u128::from(region.start);
            let first_line = relative_start / stripe;
            let last_line = (relative_end - 1) / stripe;
            let mut add = |line: u128, length: u128, repetitions: u128| {
                add_ranges(
                    destinations
                        .entry(members[(line % member_count) as usize].device.0)
                        .or_default(),
                    length,
                    repetitions,
                    maximum,
                )
            };
            if first_line == last_line {
                add(first_line, end - start, 1)?;
                continue;
            }
            add(first_line, stripe - relative_start % stripe, 1)?;
            add(last_line, (relative_end - 1) % stripe + 1, 1)?;
            // Full stripes strictly between the two endpoint stripes. Member
            // order is explicit and need not agree with physical address order.
            let first_full = first_line + 1;
            let full_count = last_line - first_full;
            for member_index in 0..member_count {
                let distance =
                    (member_index + member_count - first_full % member_count) % member_count;
                if distance < full_count {
                    add(
                        first_full + distance,
                        stripe,
                        1 + (full_count - distance - 1) / member_count,
                    )?;
                }
            }
        }
        if mapped != range.end - range.start {
            return Err(SimError(
                "Unmapped physical bytes in analytical roofline footprint".into(),
            ));
        }
    }
    Ok(destinations)
}

#[cfg(test)]
mod tests {
    use gwr_engine::types::DeviceId;

    use super::*;
    use crate::memory::memory_map::MemoryMap;
    use crate::processing_element::tensors::dtype::DataType;
    use crate::processing_element::tensors::{Tensor, TensorView};

    #[test]
    fn analytical_traffic_summarizes_bounded_fragmented_packed_views() {
        let tensor = Tensor::new(&[4, 4], &DataType::Int4, 0).unwrap();
        let view = TensorView::new(tensor, &[4, 1], &[0, 0]).unwrap();
        let regions = [MemoryRegion {
            start: 0,
            end: 7,
            device: DeviceId(3),
        }];
        let result = summarize(&view, &regions, 64).unwrap();
        assert_eq!(result[&3][&1], 4);
    }

    #[test]
    fn analytical_traffic_summarizes_fragmented_packed_ranges_across_stripes() {
        let tensor = Tensor::new(&[4, 4], &DataType::Int4, 0).unwrap();
        let view = TensorView::new(tensor, &[4, 1], &[0, 0]).unwrap();
        let mut map = MemoryMap::new();
        map.insert_striped(
            (0..4)
                .map(|member| MemoryRegion {
                    start: member * 64,
                    end: member * 64 + 63,
                    device: DeviceId(member),
                })
                .collect(),
            1,
        )
        .unwrap();
        let result = summarize_striped(&view, &map, 64).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.values().all(|counts| counts.get(&1) == Some(&2)));
    }

    #[test]
    fn analytical_traffic_rejects_fragmented_packed_views_over_bound() {
        let tensor = Tensor::new(&[100_001, 4], &DataType::Int4, 0).unwrap();
        let view = TensorView::new(tensor, &[100_001, 1], &[0, 0]).unwrap();
        let mut map = MemoryMap::new();
        map.insert(0, 200_002, DeviceId(3)).unwrap();
        let err = summarize_striped(&view, &map, 64).unwrap_err();
        assert!(err.0.contains("at most 100000 fragmented ranges"));
    }
}
