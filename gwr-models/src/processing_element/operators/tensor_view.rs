// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::dtype::DataType;
use super::floor_sum::checked_floor_sum;
use super::partition::DimPartition;
use super::tensor::{HasShape, Shape, Tensor, checked_num_bytes};

/// A validated view into a tensor.
#[derive(Clone, Debug)]
pub struct TensorView {
    tensor: Tensor,
    shape: Shape,
    offsets: Vec<usize>,
    num_packed_bytes: usize,
    layout: TensorViewLayout,
}

impl TensorView {
    pub fn new(tensor: Tensor, shape: &[usize], offsets: &[usize]) -> Result<Self, SimError> {
        let shape = Shape::new(shape)?;
        let tensor_dims = tensor.shape().dims();
        if shape.num_dims() != tensor_dims.len() {
            return sim_error!(
                "Tensor view shape rank {} does not match tensor rank {}",
                shape.num_dims(),
                tensor_dims.len()
            );
        }
        if offsets.len() != tensor_dims.len() {
            return sim_error!(
                "Tensor view offset rank {} does not match tensor rank {}",
                offsets.len(),
                tensor_dims.len()
            );
        }

        for (dimension, ((offset, extent), tensor_extent)) in offsets
            .iter()
            .zip(shape.dims())
            .zip(tensor_dims)
            .enumerate()
        {
            let end = offset.checked_add(*extent).ok_or_else(|| {
                SimError(format!(
                    "Tensor view offset {offset} plus extent {extent} overflows"
                ))
            })?;
            if end > *tensor_extent {
                return sim_error!(
                    "Tensor view range {offset}..{end} is out of range for dimension {dimension} of size {tensor_extent}"
                );
            }
        }

        let num_packed_bytes =
            checked_num_bytes(shape.num_elements(), tensor.dtype(), "Tensor view")?;
        let offsets = offsets.to_vec();
        let layout = TensorViewLayout::new(&tensor, &shape, &offsets);
        Ok(Self {
            tensor,
            shape,
            offsets,
            num_packed_bytes,
            layout,
        })
    }

    #[must_use]
    pub fn new_full(tensor: Tensor) -> Self {
        let shape = tensor.shape().clone();
        let offsets = vec![0; tensor.num_dims()];
        let num_packed_bytes = tensor.num_bytes();
        let layout = TensorViewLayout::new(&tensor, &shape, &offsets);
        Self {
            tensor,
            shape,
            offsets,
            num_packed_bytes,
            layout,
        }
    }

    pub fn from_output_partition(
        tensor: Tensor,
        output_rank: usize,
        partition_dim: usize,
        partition_offset: usize,
        partition_num_elements: usize,
    ) -> Result<Self, SimError> {
        Self::from_output_partitions(
            tensor,
            output_rank,
            &[DimPartition {
                dim: partition_dim,
                offset: partition_offset,
                num_elements: partition_num_elements,
            }],
        )
    }

    pub fn from_output_partitions(
        tensor: Tensor,
        output_rank: usize,
        partitions: &[DimPartition],
    ) -> Result<Self, SimError> {
        let base_view = Self::new_full(tensor);
        Self::from_output_partitions_on_view(&base_view, output_rank, partitions)
    }

    pub fn from_output_partitions_on_view(
        base_view: &TensorView,
        output_rank: usize,
        partitions: &[DimPartition],
    ) -> Result<Self, SimError> {
        let view_rank = base_view.num_dims();
        let rank_pad = output_rank.saturating_sub(view_rank);
        let mut shape = base_view.shape().dims().to_vec();
        let mut offsets = base_view.offsets().to_vec();

        for partition in partitions {
            if partition.dim < rank_pad {
                continue;
            }

            let view_dim = partition.dim - rank_pad;
            if view_dim < view_rank && shape[view_dim] > 1 {
                let partition_end = partition
                    .offset
                    .checked_add(partition.num_elements)
                    .ok_or_else(|| {
                        SimError("Tensor view partition extent overflows".to_string())
                    })?;
                if partition_end > shape[view_dim] {
                    return sim_error!(
                        "Tensor view partition range {}..{} is out of range for dimension of size {}",
                        partition.offset,
                        partition_end,
                        shape[view_dim]
                    );
                }
                offsets[view_dim] =
                    offsets[view_dim]
                        .checked_add(partition.offset)
                        .ok_or_else(|| {
                            SimError("Tensor view partition offset overflows".to_string())
                        })?;
                shape[view_dim] = partition.num_elements;
            }
        }

        Self::new(base_view.tensor().clone(), &shape, &offsets)
    }

    #[must_use]
    pub fn tensor(&self) -> &Tensor {
        &self.tensor
    }

    #[must_use]
    pub fn dtype(&self) -> &DataType {
        self.tensor.dtype()
    }

    #[must_use]
    pub fn offsets(&self) -> &[usize] {
        &self.offsets
    }

    #[must_use]
    pub fn layout(&self) -> &TensorViewLayout {
        &self.layout
    }

    #[must_use]
    pub fn is_full_view(&self) -> bool {
        self.shape == *self.tensor.shape() && self.offsets.iter().all(|offset| *offset == 0)
    }

    /// Return the packed size of the view as a standalone tensor.
    #[must_use]
    pub fn num_packed_bytes(&self) -> usize {
        self.num_packed_bytes
    }

    /// Return sorted, disjoint physical address ranges touched by the view.
    pub fn address_ranges(&self) -> impl Iterator<Item = Range<u128>> + '_ {
        let base = u128::from(self.tensor.addr());
        self.layout
            .byte_ranges()
            .map(move |range| base + range.start as u128..base + range.end as u128)
    }

    /// Return the smallest physical address range containing this view.
    ///
    /// The range can contain untouched gaps. Use it to identify possible
    /// overlaps, then use [`Self::first_overlapping_byte_ranges`] when an
    /// exact answer is required.
    #[must_use]
    pub fn address_bounds(&self) -> Range<u128> {
        let base = u128::from(self.tensor.addr());
        let bounds = self.layout.byte_bounds();
        base + bounds.start as u128..base + bounds.end as u128
    }

    /// Return the bytes touched by this view inside a physical address range.
    #[must_use]
    pub fn num_access_bytes_in(&self, range: Range<u128>) -> usize {
        let base = u128::from(self.tensor.addr());
        let tensor_end = base + self.tensor.num_bytes() as u128;
        let start = range.start.max(base);
        let end = range.end.min(tensor_end);
        if start >= end {
            return 0;
        }

        let start = usize::try_from(start - base)
            .expect("tensor construction guarantees byte offsets fit in usize");
        let end = usize::try_from(end - base)
            .expect("tensor construction guarantees byte offsets fit in usize");
        self.layout.num_access_bytes_in(start..end)
    }

    /// Return the first pair of physical byte ranges touched by both views.
    #[must_use]
    pub fn first_overlapping_byte_ranges(
        &self,
        other: &Self,
    ) -> Option<(Range<u128>, Range<u128>)> {
        if !ranges_overlap(&self.address_bounds(), &other.address_bounds())
            || self.coordinates_are_disjoint(other)
            || self.byte_strides_are_disjoint(other)
        {
            return None;
        }

        let mut first_ranges = self.address_ranges();
        let mut second_ranges = other.address_ranges();
        let mut first = first_ranges.next()?;
        let mut second = second_ranges.next()?;

        loop {
            if ranges_overlap(&first, &second) {
                return Some((first, second));
            }
            if first.end <= second.start {
                first = first_ranges.next()?;
            } else {
                second = second_ranges.next()?;
            }
        }
    }

    fn coordinates_are_disjoint(&self, other: &Self) -> bool {
        self.tensor.addr() == other.tensor.addr()
            && self.tensor.dtype() == other.tensor.dtype()
            && self.tensor.shape() == other.tensor.shape()
            && self.tensor.dtype().num_bits().is_multiple_of(8)
            && self
                .offsets
                .iter()
                .zip(self.shape.dims())
                .zip(other.offsets.iter().zip(other.shape.dims()))
                .any(|((offset, extent), (other_offset, other_extent))| {
                    offset + extent <= *other_offset || other_offset + other_extent <= *offset
                })
    }

    fn byte_strides_are_disjoint(&self, other: &Self) -> bool {
        let first_progressions = self.layout.byte_start_progressions(self.tensor.addr());
        let second_progressions = other.layout.byte_start_progressions(other.tensor.addr());
        first_progressions.iter().all(|first| {
            second_progressions
                .iter()
                .all(|second| !start_progressions_may_overlap(*first, *second))
        })
    }
}

impl HasShape for TensorView {
    fn num_dims(&self) -> usize {
        self.shape.num_dims()
    }

    fn num_elements(&self) -> usize {
        self.shape.num_elements()
    }

    fn get_dim(&self, total_dims: usize, index: usize) -> usize {
        self.shape.get_dim(total_dims, index)
    }

    fn shape(&self) -> &Shape {
        &self.shape
    }
}

/// The physical layout of a tensor view, relative to its tensor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TensorViewLayout {
    first_element: usize,
    elements_per_range: usize,
    strides: Vec<TensorViewStride>,
    bits_per_element: usize,
    num_access_bytes: usize,
}

impl TensorViewLayout {
    #[must_use]
    pub fn first_element(&self) -> usize {
        self.first_element
    }

    #[must_use]
    pub fn elements_per_range(&self) -> usize {
        self.elements_per_range
    }

    #[must_use]
    pub fn strides(&self) -> &[TensorViewStride] {
        &self.strides
    }

    #[must_use]
    pub fn bits_per_element(&self) -> usize {
        self.bits_per_element
    }

    #[must_use]
    pub fn num_access_bytes(&self) -> usize {
        self.num_access_bytes
    }

    /// Return sorted, disjoint byte ranges relative to the tensor address.
    pub fn byte_ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        let ranges = self
            .element_ranges()
            .map(|range| element_range_to_byte_range(range, self.bits_per_element));

        coalesce_ranges(ranges)
    }

    fn element_ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        let num_ranges = self.strides.iter().map(|stride| stride.count).product();
        (0..num_ranges).scan(vec![0; self.strides.len()], move |coordinate, _| {
            let first_element = self.first_element
                + coordinate
                    .iter()
                    .zip(&self.strides)
                    .map(|(index, stride)| index * stride.stride_elements)
                    .sum::<usize>();
            let range = first_element..first_element + self.elements_per_range;

            for dim in (0..coordinate.len()).rev() {
                coordinate[dim] += 1;
                if coordinate[dim] < self.strides[dim].count {
                    break;
                }
                coordinate[dim] = 0;
            }
            Some(range)
        })
    }

    fn new(tensor: &Tensor, shape: &Shape, offsets: &[usize]) -> Self {
        let tensor_dims = tensor.shape().dims();
        let view_dims = shape.dims();
        let rank = shape.num_dims();

        let mut contiguous_start = rank.saturating_sub(1);
        while contiguous_start > 0
            && offsets[contiguous_start] == 0
            && view_dims[contiguous_start] == tensor_dims[contiguous_start]
        {
            contiguous_start -= 1;
        }

        let mut stride_elements = 1usize;
        let mut tensor_strides = vec![1; rank];
        for dim in (0..rank).rev() {
            tensor_strides[dim] = stride_elements;
            stride_elements = stride_elements
                .checked_mul(tensor_dims[dim])
                .expect("tensor construction guarantees element strides fit in usize");
        }

        let strides = view_dims[..contiguous_start]
            .iter()
            .zip(tensor_strides)
            .map(|(count, stride_elements)| TensorViewStride {
                count: *count,
                stride_elements,
            })
            .collect::<Vec<_>>();
        let first_element = tensor_dims
            .iter()
            .zip(offsets)
            .fold(0usize, |flat, (dim, offset)| flat * dim + offset);
        let elements_per_range = view_dims[contiguous_start..].iter().product();
        let bits_per_element = tensor.dtype().num_bits();
        let num_access_bytes = access_byte_count(
            first_element,
            elements_per_range,
            &strides,
            bits_per_element,
        );

        Self {
            first_element,
            elements_per_range,
            strides,
            bits_per_element,
            num_access_bytes,
        }
    }

    /// Return the bytes touched inside a tensor-relative byte range.
    #[must_use]
    fn num_access_bytes_in(&self, range: Range<usize>) -> usize {
        bytes_in_range(
            self.first_element,
            self.elements_per_range,
            &self.strides,
            self.bits_per_element,
            &range,
        )
    }

    fn byte_bounds(&self) -> Range<usize> {
        layout_bounds(
            self.first_element,
            self.elements_per_range,
            &self.strides,
            self.bits_per_element,
        )
    }
}

/// One repeated dimension in a tensor-view layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TensorViewStride {
    count: usize,
    stride_elements: usize,
}

impl TensorViewStride {
    #[must_use]
    pub fn count(&self) -> usize {
        self.count
    }

    #[must_use]
    pub fn stride_elements(&self) -> usize {
        self.stride_elements
    }
}

fn access_byte_count(
    first_element: usize,
    elements_per_range: usize,
    strides: &[TensorViewStride],
    bits_per_element: usize,
) -> usize {
    const INVARIANT: &str = "tensor construction guarantees access sizes fit in usize";

    let num_ranges = strides
        .iter()
        .try_fold(1usize, |count, stride| count.checked_mul(stride.count))
        .expect(INVARIANT);
    if bits_per_element.is_multiple_of(8) {
        return elements_per_range
            .checked_mul(bits_per_element / 8)
            .and_then(|bytes| bytes.checked_mul(num_ranges))
            .expect(INVARIANT);
    }

    debug_assert_eq!(bits_per_element, 4);
    let mut num_bytes = elements_per_range
        .div_ceil(2)
        .checked_mul(num_ranges)
        .expect(INVARIANT);
    if elements_per_range.is_multiple_of(2) {
        num_bytes = num_bytes
            .checked_add(odd_range_count(first_element, strides))
            .expect(INVARIANT);
    }
    num_bytes
}

fn bytes_in_range(
    first_element: usize,
    elements_per_range: usize,
    strides: &[TensorViewStride],
    bits_per_element: usize,
    selected: &Range<usize>,
) -> usize {
    let bounds = layout_bounds(first_element, elements_per_range, strides, bits_per_element);
    if !ranges_overlap(&bounds, selected) {
        return 0;
    }
    if selected.start <= bounds.start && selected.end >= bounds.end {
        return access_byte_count(first_element, elements_per_range, strides, bits_per_element);
    }
    let Some((stride, inner_strides)) = strides.split_first() else {
        return bounds.end.min(selected.end) - bounds.start.max(selected.start);
    };

    let child_bounds = |index| {
        layout_bounds(
            first_element + index * stride.stride_elements,
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

    let mut total = bytes_in_range(
        first_element + first * stride.stride_elements,
        elements_per_range,
        inner_strides,
        bits_per_element,
        selected,
    );
    if end - first == 1 {
        return total;
    }

    total += bytes_in_range(
        first_element + (end - 1) * stride.stride_elements,
        elements_per_range,
        inner_strides,
        bits_per_element,
        selected,
    );
    let middle_count = end - first - 2;
    if middle_count > 0 {
        let mut middle_strides = strides.to_vec();
        middle_strides[0].count = middle_count;
        total += access_byte_count(
            first_element + (first + 1) * stride.stride_elements,
            elements_per_range,
            &middle_strides,
            bits_per_element,
        );
    }
    total
}

fn layout_bounds(
    first_element: usize,
    elements_per_range: usize,
    strides: &[TensorViewStride],
    bits_per_element: usize,
) -> Range<usize> {
    let last_range_start = strides.iter().fold(first_element, |start, stride| {
        start + (stride.count - 1) * stride.stride_elements
    });
    element_range_to_byte_range(
        first_element..last_range_start + elements_per_range,
        bits_per_element,
    )
}

fn lower_bound(count: usize, predicate: impl Fn(usize) -> bool) -> usize {
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

fn odd_range_count(first_element: usize, strides: &[TensorViewStride]) -> usize {
    const INVARIANT: &str = "tensor construction guarantees access sizes fit in usize";

    let (mut even, mut odd) = if first_element.is_multiple_of(2) {
        (1usize, 0usize)
    } else {
        (0usize, 1usize)
    };
    for stride in strides {
        if stride.stride_elements.is_multiple_of(2) {
            even = even.checked_mul(stride.count).expect(INVARIANT);
            odd = odd.checked_mul(stride.count).expect(INVARIANT);
            continue;
        }

        let even_coordinates = stride.count.div_ceil(2);
        let odd_coordinates = stride.count / 2;
        let next_even = even
            .checked_mul(even_coordinates)
            .and_then(|value| {
                odd.checked_mul(odd_coordinates)
                    .and_then(|odd_value| value.checked_add(odd_value))
            })
            .expect(INVARIANT);
        let next_odd = even
            .checked_mul(odd_coordinates)
            .and_then(|value| {
                odd.checked_mul(even_coordinates)
                    .and_then(|odd_value| value.checked_add(odd_value))
            })
            .expect(INVARIANT);
        even = next_even;
        odd = next_odd;
    }
    odd
}

fn element_range_to_byte_range(
    element_range: Range<usize>,
    bits_per_element: usize,
) -> Range<usize> {
    const INVARIANT: &str = "tensor construction guarantees byte offsets fit in usize";

    let start_bit = element_range.start as u128 * bits_per_element as u128;
    let end_bit = element_range.end as u128 * bits_per_element as u128;
    let start_byte = usize::try_from(start_bit / 8).expect(INVARIANT);
    let end_byte = usize::try_from(end_bit.div_ceil(8)).expect(INVARIANT);
    start_byte..end_byte
}

fn coalesce_ranges(
    ranges: impl Iterator<Item = Range<usize>>,
) -> impl Iterator<Item = Range<usize>> {
    let mut ranges = ranges.peekable();
    std::iter::from_fn(move || {
        let mut range = ranges.next()?;
        while ranges.peek().is_some_and(|next| next.start <= range.end) {
            let next = ranges.next().expect("peeked range is present");
            range.end = range.end.max(next.end);
        }
        Some(range)
    })
}

fn ranges_overlap<T: Ord>(first: &Range<T>, second: &Range<T>) -> bool {
    first.start < second.end && second.start < first.end
}

// A finite arithmetic progression containing every possible start byte for
// one range. It may contain unreachable starts: that can only prevent the
// disjointness shortcut from succeeding, after which the exact ranges are
// compared.
#[derive(Clone, Copy, Debug)]
struct ByteStartProgression {
    first_start: i128,
    last_start: i128,
    stride: i128,
    num_bytes_per_range: i128,
}

impl TensorViewLayout {
    fn byte_start_progressions(&self, base_address: u64) -> Vec<ByteStartProgression> {
        if self.bits_per_element.is_multiple_of(8) {
            return vec![self.byte_aligned_start_progression(base_address)];
        }
        self.packed_start_progressions(base_address)
    }

    fn byte_aligned_start_progression(&self, base_address: u64) -> ByteStartProgression {
        let bytes_per_element = self.bits_per_element / 8;
        let byte_offset = |element: usize| {
            element
                .checked_mul(bytes_per_element)
                .expect("tensor construction guarantees byte offsets fit in usize")
        };
        let stride = self
            .strides
            .iter()
            .filter(|stride| stride.count > 1)
            .map(|stride| byte_offset(stride.stride_elements))
            .fold(0, gcd);
        ByteStartProgression {
            first_start: i128::from(base_address) + byte_offset(self.first_element) as i128,
            last_start: i128::from(base_address) + byte_offset(self.last_range_element()) as i128,
            stride: stride as i128,
            num_bytes_per_range: byte_offset(self.elements_per_range) as i128,
        }
    }

    fn packed_start_progressions(&self, base_address: u64) -> Vec<ByteStartProgression> {
        debug_assert_eq!(self.bits_per_element, 4);
        let base_address = i128::from(base_address);
        let active_strides = self
            .strides
            .iter()
            .filter(|stride| stride.count > 1)
            .map(|stride| stride.stride_elements)
            .collect::<Vec<_>>();

        // A 4-bit range can start in either half of a byte. The smallest odd
        // element stride gives the first reachable start with the opposite
        // parity. Within one parity, an even stride advances by half that
        // stride in bytes, two steps along an odd stride advance by the full
        // stride, and exchanging steps between two odd strides advances by
        // half their difference. Their GCD therefore divides every reachable
        // byte-start difference within that parity.
        let smallest_odd_stride = active_strides
            .iter()
            .copied()
            .filter(|stride| !stride.is_multiple_of(2))
            .min();
        let mut stride = active_strides
            .iter()
            .copied()
            .filter(|stride| stride.is_multiple_of(2))
            .map(|stride| stride / 2)
            .fold(0, gcd);
        if let Some(smallest_odd_stride) = smallest_odd_stride {
            stride = gcd(stride, smallest_odd_stride);
            for other in active_strides
                .iter()
                .copied()
                .filter(|value| !value.is_multiple_of(2) && *value != smallest_odd_stride)
            {
                stride = gcd(stride, smallest_odd_stride.abs_diff(other) / 2);
            }
        }

        let last_element = self.last_range_element();
        let progression = |first_element: usize| {
            let parity = first_element % 2;
            let range_elements = self
                .elements_per_range
                .checked_add(parity)
                .expect("tensor construction guarantees access sizes fit in usize");
            let first = base_address + (first_element / 2) as i128;
            let stride = stride as i128;
            let upper = base_address + (last_element / 2) as i128;
            ByteStartProgression {
                first_start: first,
                last_start: if stride == 0 {
                    first
                } else {
                    first + (upper - first).div_euclid(stride) * stride
                },
                stride,
                num_bytes_per_range: range_elements.div_ceil(2) as i128,
            }
        };
        let mut progressions = vec![progression(self.first_element)];
        if let Some(smallest_odd_stride) = smallest_odd_stride {
            progressions.push(progression(
                self.first_element
                    .checked_add(smallest_odd_stride)
                    .expect("tensor construction guarantees element offsets fit in usize"),
            ));
        }
        progressions
    }

    fn last_range_element(&self) -> usize {
        self.strides
            .iter()
            .fold(self.first_element, |start, stride| {
                start
                    .checked_add(
                        (stride.count - 1)
                            .checked_mul(stride.stride_elements)
                            .expect("tensor construction guarantees element offsets fit in usize"),
                    )
                    .expect("tensor construction guarantees element offsets fit in usize")
            })
    }
}

fn start_progressions_may_overlap(
    first_progression: ByteStartProgression,
    second_progression: ByteStartProgression,
) -> bool {
    if first_progression.first_start
        >= second_progression.last_start + second_progression.num_bytes_per_range
        || second_progression.first_start
            >= first_progression.last_start + first_progression.num_bytes_per_range
    {
        return false;
    }

    bounded_progression_ranges_overlap(first_progression, second_progression).unwrap_or(true)
}

fn bounded_progression_ranges_overlap(
    first_progression: ByteStartProgression,
    second_progression: ByteStartProgression,
) -> Option<bool> {
    let min_start_difference = 1i128.checked_sub(second_progression.num_bytes_per_range)?;
    let max_start_difference = first_progression.num_bytes_per_range.checked_sub(1)?;

    match (first_progression.stride, second_progression.stride) {
        (0, _) => progression_has_value_between(
            second_progression,
            first_progression
                .first_start
                .checked_add(min_start_difference)?,
            first_progression
                .first_start
                .checked_add(max_start_difference)?,
        ),
        (_, 0) => progression_has_value_between(
            first_progression,
            second_progression
                .first_start
                .checked_sub(max_start_difference)?,
            second_progression
                .first_start
                .checked_sub(min_start_difference)?,
        ),
        (_, _) => {
            // A pair of ranges overlaps when the difference between their
            // start addresses lies in the inclusive interval above. Count the
            // finite progression pairs at or below each end of that interval;
            // a larger count at the upper end proves that at least one pair
            // overlaps without visiting the individual ranges.
            let before_interval = count_progression_differences_at_most(
                first_progression,
                second_progression,
                min_start_difference.checked_sub(1)?,
            )?;
            let through_interval = count_progression_differences_at_most(
                first_progression,
                second_progression,
                max_start_difference,
            )?;
            Some(through_interval > before_interval)
        }
    }
}

fn progression_has_value_between(
    progression: ByteStartProgression,
    lower: i128,
    upper: i128,
) -> Option<bool> {
    let lower = lower.max(progression.first_start);
    let upper = upper.min(progression.last_start);
    if lower > upper {
        return Some(false);
    }
    if progression.stride == 0 {
        return Some(true);
    }

    let offset = lower.checked_sub(progression.first_start)?;
    let steps = offset.checked_add(progression.stride.checked_sub(1)?)? / progression.stride;
    let first_value = progression
        .stride
        .checked_mul(steps)
        .and_then(|offset| progression.first_start.checked_add(offset))?;
    Some(first_value <= upper)
}

fn count_progression_differences_at_most(
    first_progression: ByteStartProgression,
    second_progression: ByteStartProgression,
    max_start_difference: i128,
) -> Option<u128> {
    debug_assert_ne!(first_progression.stride, 0);
    debug_assert_ne!(second_progression.stride, 0);

    // For each start in `first_progression`, count the starts in
    // `second_progression` for which
    //
    // second_start - first_start <= max_start_difference.
    //
    // The count is initially zero, then a floor progression, and finally the
    // complete second progression. Find those boundaries arithmetically and
    // sum only the middle section.
    let first_count = progression_count(first_progression)?;
    let second_count = progression_count(second_progression)?;
    let first_stride = u128::try_from(first_progression.stride).ok()?;
    let second_stride = u128::try_from(second_progression.stride).ok()?;
    let first_numerator = max_start_difference
        .checked_add(first_progression.first_start)?
        .checked_sub(second_progression.first_start)?;
    let partial_start = first_index_at_least(first_numerator, first_stride, 0, first_count)?;
    let complete_threshold = second_progression
        .last_start
        .checked_sub(second_progression.first_start)
        .and_then(|span| u128::try_from(span).ok())?;
    let complete_start = first_index_at_least(
        first_numerator,
        first_stride,
        i128::try_from(complete_threshold).ok()?,
        first_count,
    )?;

    let partial_count = complete_start.checked_sub(partial_start)?;
    let partial_pairs = if partial_count == 0 {
        0
    } else {
        let partial_offset = first_stride
            .checked_mul(partial_start)
            .and_then(|offset| i128::try_from(offset).ok())
            .and_then(|offset| first_numerator.checked_add(offset))
            .and_then(|offset| u128::try_from(offset).ok())?;
        checked_floor_sum(partial_count, second_stride, first_stride, partial_offset)?
            .checked_add(partial_count)?
    };
    let complete_pairs = first_count
        .checked_sub(complete_start)?
        .checked_mul(second_count)?;
    partial_pairs.checked_add(complete_pairs)
}

fn progression_count(progression: ByteStartProgression) -> Option<u128> {
    if progression.stride == 0 {
        return Some(1);
    }

    let span = progression
        .last_start
        .checked_sub(progression.first_start)?;
    u128::try_from(span / progression.stride)
        .ok()?
        .checked_add(1)
}

fn first_index_at_least(
    first_value: i128,
    stride: u128,
    target: i128,
    count: u128,
) -> Option<u128> {
    if stride == 0 {
        return None;
    }
    if first_value >= target {
        return Some(0);
    }

    let difference = target
        .checked_sub(first_value)
        .and_then(|difference| u128::try_from(difference).ok())?;
    Some(difference.div_ceil(stride).min(count))
}

#[cfg(test)]
fn progression_contains(progression: ByteStartProgression, value: i128) -> bool {
    value >= progression.first_start
        && value <= progression.last_start
        && (progression.stride == 0
            || (value - progression.first_start).rem_euclid(progression.stride) == 0)
}

fn gcd(mut first: usize, mut second: usize) -> usize {
    while second != 0 {
        (first, second) = (second, first % second);
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tensor(dims: &[usize], dtype: DataType) -> Tensor {
        Tensor::new(dims, &dtype, 0).unwrap()
    }

    fn exact_overlap(
        first: &TensorView,
        second: &TensorView,
    ) -> Option<(Range<u128>, Range<u128>)> {
        let mut first_ranges = first.address_ranges();
        let mut second_ranges = second.address_ranges();
        let mut first = first_ranges.next()?;
        let mut second = second_ranges.next()?;

        loop {
            if ranges_overlap(&first, &second) {
                return Some((first, second));
            }
            if first.end <= second.start {
                first = first_ranges.next()?;
            } else {
                second = second_ranges.next()?;
            }
        }
    }

    fn three_dimensional_views(tensor: &Tensor) -> Vec<TensorView> {
        let dims = tensor.shape().dims();
        assert_eq!(dims.len(), 3);
        let mut views = Vec::new();
        for first_offset in 0..dims[0] {
            for second_offset in 0..dims[1] {
                for third_offset in 0..dims[2] {
                    for first_extent in 1..=dims[0] - first_offset {
                        for second_extent in 1..=dims[1] - second_offset {
                            for third_extent in 1..=dims[2] - third_offset {
                                views.push(
                                    TensorView::new(
                                        tensor.clone(),
                                        &[first_extent, second_extent, third_extent],
                                        &[first_offset, second_offset, third_offset],
                                    )
                                    .unwrap(),
                                );
                            }
                        }
                    }
                }
            }
        }
        views
    }

    fn progression(
        first: i128,
        stride: i128,
        count: i128,
        num_bytes_per_range: i128,
    ) -> ByteStartProgression {
        assert!(count > 0);
        assert!(num_bytes_per_range > 0);
        let count = if stride == 0 { 1 } else { count };
        ByteStartProgression {
            first_start: first,
            last_start: first + stride * (count - 1),
            stride,
            num_bytes_per_range,
        }
    }

    fn exact_progression_ranges_overlap(
        first: ByteStartProgression,
        second: ByteStartProgression,
    ) -> bool {
        let first_count = i128::try_from(progression_count(first).unwrap()).unwrap();
        let second_count = i128::try_from(progression_count(second).unwrap()).unwrap();
        (0..first_count).any(|first_index| {
            let first_start = first.first_start + first_index * first.stride;
            (0..second_count).any(|second_index| {
                let second_start = second.first_start + second_index * second.stride;
                first_start < second_start + second.num_bytes_per_range
                    && second_start < first_start + first.num_bytes_per_range
            })
        })
    }

    #[test]
    fn rejects_invalid_views() {
        let tensor = tensor(&[4, 4], DataType::Fp32);
        assert!(TensorView::new(tensor.clone(), &[1], &[0, 0]).is_err());
        assert!(TensorView::new(tensor.clone(), &[1, 1], &[0]).is_err());
        assert!(TensorView::new(tensor.clone(), &[1, 1], &[usize::MAX, 0]).is_err());
        assert!(TensorView::new(tensor, &[3, 1], &[2, 0]).is_err());
    }

    #[test]
    fn describes_strided_packed_views() {
        let tensor = tensor(&[4, 4], DataType::Int4);
        let view = TensorView::new(tensor, &[3, 1], &[1, 1]).unwrap();

        assert_eq!(view.layout().first_element(), 5);
        assert_eq!(view.layout().elements_per_range(), 1);
        assert_eq!(view.layout().bits_per_element(), 4);
        assert_eq!(view.layout().num_access_bytes(), 3);
        assert_eq!(view.layout().strides().len(), 1);
        assert_eq!(view.layout().strides()[0].count(), 3);
        assert_eq!(view.layout().strides()[0].stride_elements(), 4);
        assert_eq!(
            view.layout().byte_ranges().collect::<Vec<_>>(),
            vec![2..3, 4..5, 6..7]
        );
        assert_eq!(view.address_bounds(), 2..7);
    }

    #[test]
    fn keeps_absolute_ranges_at_the_end_of_the_address_space() {
        let tensor = Tensor::new(&[2], &DataType::Int8, u64::MAX - 1).unwrap();
        let view = TensorView::new_full(tensor);
        assert_eq!(
            view.address_ranges().collect::<Vec<_>>(),
            vec![u128::from(u64::MAX) - 1..u128::from(u64::MAX) + 1]
        );
    }

    #[test]
    fn calculates_access_bytes_without_enumerating_ranges() {
        let tensor = tensor(&[100_000_000, 2], DataType::Int8);
        let view = TensorView::new(tensor, &[100_000_000, 1], &[0, 0]).unwrap();

        assert_eq!(view.layout().num_access_bytes(), 100_000_000);
        assert_eq!(
            view.layout().byte_ranges().take(3).collect::<Vec<_>>(),
            vec![0..1, 2..3, 4..5]
        );
    }

    #[test]
    fn analytical_sizes_match_exact_ranges_for_small_views() {
        for dtype in [DataType::Int4, DataType::Int8, DataType::Fp32] {
            for rows in 1..=5 {
                for columns in 1..=5 {
                    let tensor = tensor(&[rows, columns], dtype);
                    for row_offset in 0..rows {
                        for column_offset in 0..columns {
                            for view_rows in 1..=rows - row_offset {
                                for view_columns in 1..=columns - column_offset {
                                    let view = TensorView::new(
                                        tensor.clone(),
                                        &[view_rows, view_columns],
                                        &[row_offset, column_offset],
                                    )
                                    .unwrap();
                                    let exact = view
                                        .layout()
                                        .byte_ranges()
                                        .map(|range| range.len())
                                        .sum::<usize>();
                                    assert_eq!(view.layout().num_access_bytes(), exact);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn analytical_intersections_match_exact_ranges_for_small_views() {
        for dtype in [DataType::Int4, DataType::Int8, DataType::Fp32] {
            for rows in 1..=4 {
                for columns in 1..=4 {
                    let tensor = tensor(&[rows, columns], dtype);
                    for row_offset in 0..rows {
                        for column_offset in 0..columns {
                            let view = TensorView::new(
                                tensor.clone(),
                                &[rows - row_offset, columns - column_offset],
                                &[row_offset, column_offset],
                            )
                            .unwrap();
                            for start in 0..=tensor.num_bytes() {
                                for end in start..=tensor.num_bytes() {
                                    let exact = view
                                        .layout()
                                        .byte_ranges()
                                        .map(|range| {
                                            range
                                                .end
                                                .min(end)
                                                .saturating_sub(range.start.max(start))
                                        })
                                        .sum::<usize>();
                                    assert_eq!(
                                        view.layout().num_access_bytes_in(start..end),
                                        exact,
                                        "{dtype:?} {rows}x{columns} at {row_offset},{column_offset} in {start}..{end}",
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn intersects_large_strided_views_without_enumerating_ranges() {
        let tensor = tensor(&[100_000_000, 2], DataType::Int8);
        let view = TensorView::new(tensor, &[100_000_000, 1], &[0, 0]).unwrap();

        assert_eq!(
            view.layout().num_access_bytes_in(50_000_000..150_000_000),
            50_000_000
        );
    }

    #[test]
    fn finds_exact_overlaps() {
        let tensor = tensor(&[4, 4], DataType::Int8);
        let first = TensorView::new(tensor.clone(), &[4, 1], &[0, 0]).unwrap();
        let second = TensorView::new(tensor.clone(), &[4, 1], &[0, 1]).unwrap();
        let overlapping = TensorView::new(tensor, &[2, 1], &[1, 0]).unwrap();

        assert_eq!(first.first_overlapping_byte_ranges(&second), None);
        assert_eq!(
            first.first_overlapping_byte_ranges(&overlapping),
            Some((4..5, 4..5))
        );
    }

    #[test]
    fn bounded_progression_overlap_matches_exact_ranges() {
        for first_start in 0..=4 {
            for second_start in 0..=4 {
                for first_stride in 0..=4 {
                    for second_stride in 0..=4 {
                        for first_count in 1..=5 {
                            for second_count in 1..=5 {
                                for first_num_bytes in 1..=4 {
                                    for second_num_bytes in 1..=4 {
                                        let first = progression(
                                            first_start,
                                            first_stride,
                                            first_count,
                                            first_num_bytes,
                                        );
                                        let second = progression(
                                            second_start,
                                            second_stride,
                                            second_count,
                                            second_num_bytes,
                                        );
                                        assert_eq!(
                                            bounded_progression_ranges_overlap(first, second),
                                            Some(exact_progression_ranges_overlap(first, second)),
                                            "first={first:?}, second={second:?}",
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn bounded_progression_overlap_includes_the_final_physical_byte() {
        let final_byte = i128::from(u64::MAX);
        let first = progression(final_byte - 1, 0, 1, 2);
        let overlapping = progression(final_byte, 0, 1, 1);
        let disjoint = progression(final_byte - 2, 0, 1, 1);

        assert_eq!(
            bounded_progression_ranges_overlap(first, overlapping),
            Some(true)
        );
        assert_eq!(
            bounded_progression_ranges_overlap(disjoint, overlapping),
            Some(false)
        );
    }

    #[test]
    fn packed_elements_in_one_byte_overlap() {
        let tensor = tensor(&[2], DataType::Int4);
        let first = TensorView::new(tensor.clone(), &[1], &[0]).unwrap();
        let second = TensorView::new(tensor, &[1], &[1]).unwrap();

        assert_eq!(
            first.first_overlapping_byte_ranges(&second),
            Some((0..1, 0..1))
        );
    }

    #[test]
    fn finds_overlap_from_multiple_odd_packed_strides() {
        let packed_tensor = Tensor::new(&[2, 3, 3], &DataType::Int4, 0).unwrap();
        let packed = TensorView::new(packed_tensor, &[2, 2, 1], &[0, 0, 0]).unwrap();
        let byte_tensor = Tensor::new(&[1], &DataType::Int8, 1).unwrap();
        let byte = TensorView::new_full(byte_tensor);

        assert_eq!(
            packed.address_ranges().collect::<Vec<_>>(),
            vec![0..2, 4..5, 6..7]
        );
        assert_eq!(
            packed.first_overlapping_byte_ranges(&byte),
            Some((0..2, 1..2))
        );
    }

    #[test]
    fn packed_start_progressions_cover_every_range() {
        for dtype in [DataType::Fp4, DataType::Int4] {
            let tensor = Tensor::new(&[2, 3, 3], &dtype, 0).unwrap();
            for view in three_dimensional_views(&tensor) {
                let layout = view.layout();
                let progressions = layout.byte_start_progressions(0);
                for range in layout
                    .element_ranges()
                    .map(|range| element_range_to_byte_range(range, layout.bits_per_element))
                {
                    assert!(progressions.iter().any(|progression| {
                        progression_contains(*progression, range.start as i128)
                            && progression.num_bytes_per_range >= range.len() as i128
                    }));
                }
            }
        }
    }

    #[test]
    fn packed_stride_proof_is_conservative_for_small_three_dimensional_views() {
        for dtype in [DataType::Fp4, DataType::Int4] {
            let first_tensor = Tensor::new(&[2, 3, 3], &dtype, 0).unwrap();
            let first_views = three_dimensional_views(&first_tensor);
            for second_address in 0..=1 {
                let second_tensor = Tensor::new(&[2, 3, 3], &dtype, second_address).unwrap();
                let second_views = three_dimensional_views(&second_tensor);
                for first in &first_views {
                    for second in &second_views {
                        let exact = exact_overlap(first, second);
                        if first.byte_strides_are_disjoint(second) {
                            assert_eq!(exact, None);
                        }
                        assert_eq!(first.first_overlapping_byte_ranges(second), exact);
                    }
                }
            }
        }
    }

    #[test]
    fn proves_large_shifted_byte_strides_are_disjoint() {
        let first_tensor = Tensor::new(&[200_000_000, 2], &DataType::Int8, 0).unwrap();
        let second_tensor = Tensor::new(&[200_000_000, 2], &DataType::Int8, 1).unwrap();
        let first = TensorView::new(first_tensor, &[200_000_000, 1], &[0, 0]).unwrap();
        let second = TensorView::new(second_tensor, &[200_000_000, 1], &[0, 0]).unwrap();

        assert!(ranges_overlap(
            &first.address_bounds(),
            &second.address_bounds()
        ));
        assert!(first.byte_strides_are_disjoint(&second));
        assert_eq!(first.first_overlapping_byte_ranges(&second), None);
    }

    #[test]
    fn proves_finite_byte_strides_are_disjoint() {
        const STRIDE: usize = 1_000_000_000;
        const COUNT: usize = STRIDE / 2;

        let first_tensor = Tensor::new(&[COUNT, STRIDE], &DataType::Int8, 0).unwrap();
        let second_tensor = Tensor::new(&[COUNT, STRIDE + 1], &DataType::Int8, 1).unwrap();
        let first = TensorView::new(first_tensor, &[COUNT, 1], &[0, 0]).unwrap();
        let second = TensorView::new(second_tensor, &[COUNT, 1], &[0, 0]).unwrap();

        assert!(ranges_overlap(
            &first.address_bounds(),
            &second.address_bounds()
        ));
        assert!(first.byte_strides_are_disjoint(&second));
        assert_eq!(first.first_overlapping_byte_ranges(&second), None);
    }

    #[test]
    fn proves_wide_finite_byte_strides_are_disjoint() {
        const COUNT: usize = 100_000_000;

        let first_tensor = Tensor::new(&[COUNT, 1_000_000_001], &DataType::Int8, 0).unwrap();
        let second_tensor = Tensor::new(&[COUNT, 1_000_000_005], &DataType::Int8, 2).unwrap();
        let first = TensorView::new(first_tensor, &[COUNT, 2], &[0, 0]).unwrap();
        let second = TensorView::new(second_tensor, &[COUNT, 2], &[0, 0]).unwrap();

        assert!(ranges_overlap(
            &first.address_bounds(),
            &second.address_bounds()
        ));
        assert!(first.byte_strides_are_disjoint(&second));
        assert_eq!(first.first_overlapping_byte_ranges(&second), None);
    }

    #[test]
    fn proves_large_packed_strides_are_disjoint() {
        let tensor = Tensor::new(&[200_000_000, 4], &DataType::Int4, 0).unwrap();
        let first = TensorView::new(tensor.clone(), &[200_000_000, 1], &[0, 0]).unwrap();
        let second = TensorView::new(tensor, &[200_000_000, 1], &[0, 2]).unwrap();

        assert!(ranges_overlap(
            &first.address_bounds(),
            &second.address_bounds()
        ));
        assert!(first.byte_strides_are_disjoint(&second));
        assert_eq!(first.first_overlapping_byte_ranges(&second), None);
    }

    #[test]
    fn stride_proof_does_not_hide_shifted_overlaps() {
        let first_tensor = Tensor::new(&[10, 2], &DataType::Int8, 0).unwrap();
        let second_tensor = Tensor::new(&[10, 2], &DataType::Int8, 2).unwrap();
        let first = TensorView::new(first_tensor, &[10, 1], &[0, 0]).unwrap();
        let second = TensorView::new(second_tensor, &[10, 1], &[0, 0]).unwrap();

        assert!(!first.byte_strides_are_disjoint(&second));
        assert_eq!(
            first.first_overlapping_byte_ranges(&second),
            Some((2..3, 2..3))
        );
    }

    #[test]
    fn stride_disjointness_proof_is_conservative_for_small_views() {
        for dtype in [DataType::Int4, DataType::Int8, DataType::Fp32] {
            for columns in 1..=5 {
                let tensor_bytes = Tensor::new(&[4, columns], &dtype, 0).unwrap().num_bytes();
                for first_address in 0..=2 {
                    for second_address in 0..=2 {
                        let first_tensor =
                            Tensor::new(&[4, columns], &dtype, first_address).unwrap();
                        let second_tensor =
                            Tensor::new(&[4, columns], &dtype, second_address).unwrap();
                        for first_column in 0..columns {
                            for second_column in 0..columns {
                                let first = TensorView::new(
                                    first_tensor.clone(),
                                    &[4, 1],
                                    &[0, first_column],
                                )
                                .unwrap();
                                let second = TensorView::new(
                                    second_tensor.clone(),
                                    &[4, 1],
                                    &[0, second_column],
                                )
                                .unwrap();
                                let exact = exact_overlap(&first, &second);

                                if first.byte_strides_are_disjoint(&second) {
                                    assert_eq!(exact, None);
                                }
                                assert_eq!(
                                    first.first_overlapping_byte_ranges(&second),
                                    exact,
                                    "{dtype:?} {columns} columns, {tensor_bytes} bytes, addresses {first_address} and {second_address}, columns {first_column} and {second_column}",
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
