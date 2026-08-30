// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

pub(crate) use crate::address::{AddressRange, merge_ranges, range_union_length};
use crate::model::{TensorAccess, TensorStride, TensorSummary};

impl TensorAccess {
    pub(crate) fn num_bytes_in(&self, tensor_addr: u64, selected: AddressRange) -> u128 {
        let base = u128::from(tensor_addr);
        if selected.end <= base {
            return 0;
        }
        bytes_in_access_range(
            u128::from(self.first_element),
            u128::from(self.elements_per_range),
            &self.strides,
            u128::from(self.bits_per_element),
            AddressRange::from_bounds(
                selected.start.saturating_sub(base),
                selected.end.saturating_sub(base),
            ),
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TensorLayout {
    pub(crate) tensor_index: usize,
    pub(crate) address: u64,
    pub(crate) bytes: u64,
}

#[derive(Debug)]
pub(crate) struct MemoryRegion {
    pub(crate) start: u128,
    pub(crate) end: u128,
    pub(crate) gap_before: u128,
    pub(crate) allocated: u128,
    pub(crate) tensors: Vec<TensorLayout>,
}

impl MemoryRegion {
    pub(crate) fn span(&self) -> u128 {
        (self.end - self.start).max(1)
    }
}

pub(crate) fn clipped_range(
    left_start: u64,
    left_bytes: u64,
    right_start: u64,
    right_bytes: u64,
) -> Option<(u64, u64)> {
    let intersection = AddressRange::new(left_start, left_bytes)
        .intersection(AddressRange::new(right_start, right_bytes))?;
    Some((
        u64::try_from(intersection.start)
            .expect("an intersection start derived from u64 addresses fits in u64"),
        u64::try_from(intersection.len())
            .expect("an intersection cannot exceed either u64 input length"),
    ))
}

pub(crate) fn build_regions(
    mut layouts: Vec<TensorLayout>,
    tensors: &[TensorSummary],
    skip_gaps: bool,
) -> Vec<MemoryRegion> {
    layouts.sort_by(|left, right| {
        left.address.cmp(&right.address).then_with(|| {
            tensors[left.tensor_index]
                .id
                .cmp(&tensors[right.tensor_index].id)
        })
    });
    let largest = layouts
        .iter()
        .map(|layout| u128::from(layout.bytes))
        .max()
        .unwrap_or(1);
    let total = range_union_length(
        layouts
            .iter()
            .map(|layout| AddressRange::new(layout.address, layout.bytes)),
    );
    let threshold = skip_gaps.then_some(largest.max(total / 64).max(4096));
    let mut regions: Vec<MemoryRegion> = Vec::new();
    for layout in layouts {
        let range = AddressRange::new(layout.address, layout.bytes);
        let gap = regions
            .last()
            .map_or(0, |region| range.start.saturating_sub(region.end));
        if regions.is_empty() || threshold.is_some_and(|threshold| gap > threshold) {
            regions.push(MemoryRegion {
                start: range.start,
                end: range.end,
                gap_before: if regions.is_empty() { 0 } else { gap },
                allocated: range.len(),
                tensors: vec![layout],
            });
        } else if let Some(region) = regions.last_mut() {
            if range.end > region.end {
                region.allocated += range.end - range.start.max(region.end);
            }
            region.end = region.end.max(range.end);
            region.tensors.push(layout);
        }
    }
    regions
}

fn bytes_in_access_range(
    first_element: u128,
    elements_per_range: u128,
    strides: &[TensorStride],
    bits_per_element: u128,
    selected: AddressRange,
) -> u128 {
    let bounds = access_bounds(first_element, elements_per_range, strides, bits_per_element);
    if bounds.intersection(selected).is_none() {
        return 0;
    }
    if selected.start <= bounds.start && selected.end >= bounds.end {
        return access_bytes(first_element, elements_per_range, strides, bits_per_element);
    }
    let Some((stride, inner_strides)) = strides.split_first() else {
        return bounds.intersection(selected).map_or(0, AddressRange::len);
    };

    let child_bounds = |index: u64| {
        access_bounds(
            first_element + u128::from(index) * u128::from(stride.stride_elements),
            elements_per_range,
            inner_strides,
            bits_per_element,
        )
    };
    let first = lower_bound(stride.count, |index| {
        child_bounds(index).end <= selected.start
    });
    let end = lower_bound(stride.count, |index| {
        child_bounds(index).start < selected.end
    });
    if first >= end {
        return 0;
    }

    let mut total = bytes_in_access_range(
        first_element + u128::from(first) * u128::from(stride.stride_elements),
        elements_per_range,
        inner_strides,
        bits_per_element,
        selected,
    );
    if end - first == 1 {
        return total;
    }
    total += bytes_in_access_range(
        first_element + u128::from(end - 1) * u128::from(stride.stride_elements),
        elements_per_range,
        inner_strides,
        bits_per_element,
        selected,
    );

    let middle_count = end - first - 2;
    if middle_count > 0 {
        let mut middle_strides = strides.to_vec();
        middle_strides[0].count = middle_count;
        total += access_bytes(
            first_element + u128::from(first + 1) * u128::from(stride.stride_elements),
            elements_per_range,
            &middle_strides,
            bits_per_element,
        );
    }
    total
}

fn access_bounds(
    first_element: u128,
    elements_per_range: u128,
    strides: &[TensorStride],
    bits_per_element: u128,
) -> AddressRange {
    let last_range_start = strides.iter().fold(first_element, |start, stride| {
        start + u128::from(stride.count - 1) * u128::from(stride.stride_elements)
    });
    element_range_to_bytes(
        first_element,
        last_range_start + elements_per_range,
        bits_per_element,
    )
}

fn element_range_to_bytes(start: u128, end: u128, bits_per_element: u128) -> AddressRange {
    AddressRange::from_bounds(
        start * bits_per_element / 8,
        (end * bits_per_element).div_ceil(8),
    )
}

fn access_bytes(
    first_element: u128,
    elements_per_range: u128,
    strides: &[TensorStride],
    bits_per_element: u128,
) -> u128 {
    let num_ranges = strides
        .iter()
        .map(|stride| u128::from(stride.count))
        .product::<u128>();
    if bits_per_element.is_multiple_of(8) {
        return elements_per_range * (bits_per_element / 8) * num_ranges;
    }

    debug_assert_eq!(bits_per_element, 4);
    let mut num_bytes = elements_per_range.div_ceil(2) * num_ranges;
    if elements_per_range.is_multiple_of(2) {
        num_bytes += odd_access_range_count(first_element, strides);
    }
    num_bytes
}

fn odd_access_range_count(first_element: u128, strides: &[TensorStride]) -> u128 {
    let (mut even, mut odd) = if first_element.is_multiple_of(2) {
        (1, 0)
    } else {
        (0, 1)
    };
    for stride in strides {
        let count = u128::from(stride.count);
        if stride.stride_elements.is_multiple_of(2) {
            even *= count;
            odd *= count;
            continue;
        }
        let even_coordinates = count.div_ceil(2);
        let odd_coordinates = count / 2;
        (even, odd) = (
            even * even_coordinates + odd * odd_coordinates,
            even * odd_coordinates + odd * even_coordinates,
        );
    }
    odd
}

fn lower_bound(count: u64, predicate: impl Fn(u64) -> bool) -> u64 {
    let mut first = 0;
    let mut len = count;
    while len > 0 {
        let half = len / 2;
        let middle = first + half;
        if predicate(middle) {
            first = middle + 1;
            len -= half + 1;
        } else {
            len = half;
        }
    }
    first
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "generator")]
    use gwr_models::processing_element::operators::dtype::DataType;
    #[cfg(feature = "generator")]
    use gwr_models::processing_element::operators::{Tensor, TensorView};

    #[cfg(feature = "generator")]
    use super::AddressRange;
    use super::{TensorLayout, build_regions, clipped_range};
    #[cfg(feature = "generator")]
    use crate::model::TensorAccess;
    use crate::model::TensorSummary;

    fn tensor(id: &str) -> TensorSummary {
        TensorSummary {
            id: id.into(),
            addr: 0,
            num_bytes: 1,
            dtype: "int8".into(),
            shape: vec![1],
            writes_by_pe: Vec::new(),
            reads_by_pe: Vec::new(),
        }
    }

    #[cfg(feature = "generator")]
    fn assert_report_intersections_match(view: &TensorView) {
        let base = view.tensor().addr();
        let access = TensorAccess::try_from(view.layout()).unwrap();
        for start in 0..=view.tensor().num_bytes() {
            for end in start..=view.tensor().num_bytes() {
                let selected = AddressRange::from_bounds(
                    u128::from(base) + start as u128,
                    u128::from(base) + end as u128,
                );
                assert_eq!(
                    access.num_bytes_in(base, selected),
                    view.num_access_bytes_in(
                        u128::from(base) + start as u128..u128::from(base) + end as u128,
                    ) as u128,
                    "{view:?} in {start}..{end}",
                );
            }
        }
    }

    #[test]
    fn clips_ranges_ending_at_the_final_physical_byte() {
        assert_eq!(clipped_range(100, 40, 120, 100), Some((120, 20)));
        assert_eq!(clipped_range(100, 40, 200, 100), None);
        assert_eq!(
            clipped_range(u64::MAX - 1, 2, u64::MAX - 1, 2),
            Some((u64::MAX - 1, 2))
        );
    }

    #[test]
    #[cfg(feature = "generator")]
    fn report_intersections_match_tensor_view_layouts() {
        const BASE: u64 = 17;
        for dtype in [DataType::Int4, DataType::Int8, DataType::Fp32] {
            for rows in 1..=4 {
                for columns in 1..=4 {
                    let tensor = Tensor::new(&[rows, columns], &dtype, BASE).unwrap();
                    for row_offset in 0..rows {
                        for column_offset in 0..columns {
                            let view = TensorView::new(
                                tensor.clone(),
                                &[rows - row_offset, columns - column_offset],
                                &[row_offset, column_offset],
                            )
                            .unwrap();
                            assert_report_intersections_match(&view);
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[cfg(feature = "generator")]
    fn report_intersections_match_multiple_odd_packed_strides() {
        const BASE: u64 = 17;
        for dtype in [DataType::Fp4, DataType::Int4] {
            let tensor = Tensor::new(&[2, 3, 3], &dtype, BASE).unwrap();
            let view = TensorView::new(tensor, &[2, 2, 1], &[0, 0, 0]).unwrap();
            assert_report_intersections_match(&view);
        }
    }

    #[test]
    fn splits_large_memory_gaps() {
        let tensors = [tensor("first"), tensor("second")];
        let regions = build_regions(
            vec![
                TensorLayout {
                    tensor_index: 0,
                    address: 0,
                    bytes: 16,
                },
                TensorLayout {
                    tensor_index: 1,
                    address: 16_384,
                    bytes: 16,
                },
            ],
            &tensors,
            true,
        );

        assert_eq!(regions.len(), 2);
        assert_eq!(regions[1].gap_before, 16_368);
    }

    #[test]
    fn preserves_the_exclusive_endpoint_above_u64_max() {
        let regions = build_regions(
            vec![TensorLayout {
                tensor_index: 0,
                address: u64::MAX - 1,
                bytes: 2,
            }],
            &[tensor("final")],
            false,
        );

        assert_eq!(regions[0].start, u128::from(u64::MAX) - 1);
        assert_eq!(regions[0].end, u128::from(u64::MAX) + 1);
        assert_eq!(regions[0].span(), 2);
        assert_eq!(regions[0].allocated, 2);
    }

    #[test]
    fn unions_aliased_tensor_allocations() {
        let regions = build_regions(
            vec![
                TensorLayout {
                    tensor_index: 0,
                    address: 0,
                    bytes: 8,
                },
                TensorLayout {
                    tensor_index: 1,
                    address: 4,
                    bytes: 8,
                },
            ],
            &[tensor("first"), tensor("second")],
            false,
        );

        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start, 0);
        assert_eq!(regions[0].end, 12);
        assert_eq!(regions[0].allocated, 12);
        assert_eq!(regions[0].tensors.len(), 2);
    }

    #[test]
    fn unions_contained_and_adjacent_tensor_allocations() {
        let regions = build_regions(
            vec![
                TensorLayout {
                    tensor_index: 0,
                    address: 0,
                    bytes: 8,
                },
                TensorLayout {
                    tensor_index: 1,
                    address: 6,
                    bytes: 2,
                },
                TensorLayout {
                    tensor_index: 2,
                    address: 8,
                    bytes: 4,
                },
            ],
            &[tensor("first"), tensor("contained"), tensor("adjacent")],
            false,
        );

        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start, 0);
        assert_eq!(regions[0].end, 12);
        assert_eq!(regions[0].allocated, 12);
        assert_eq!(regions[0].tensors.len(), 3);
    }

    #[test]
    fn sorts_memory_layouts_by_address_then_tensor_id() {
        let regions = build_regions(
            vec![
                TensorLayout {
                    tensor_index: 0,
                    address: 100,
                    bytes: 1,
                },
                TensorLayout {
                    tensor_index: 1,
                    address: 0,
                    bytes: 1,
                },
                TensorLayout {
                    tensor_index: 2,
                    address: 0,
                    bytes: 1,
                },
            ],
            &[tensor("z"), tensor("b"), tensor("a")],
            false,
        );

        let order = regions[0]
            .tensors
            .iter()
            .map(|layout| layout.tensor_index)
            .collect::<Vec<_>>();
        assert_eq!(regions[0].start, 0);
        assert_eq!(regions[0].end, 101);
        assert_eq!(order, vec![2, 1, 0]);
    }
}
