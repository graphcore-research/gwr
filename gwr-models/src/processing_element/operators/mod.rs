// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! The Operators define what operations a Processing Element can perform

use std::fmt::Display;
use std::ops::Range;
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};

use crate::processing_element::operators::dtype::DataType;
use crate::processing_element::{ComputeCapabilities, MachineOpCounts};

pub mod dtype;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpansionDirection {
    Backward,
    Forward,
}

#[must_use]
pub fn shape_string(dims: &[usize]) -> String {
    dims.iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join("×")
}

pub trait HasShape {
    /// Return the number of dimensions
    #[must_use]
    fn num_dims(&self) -> usize;

    /// Return the number of elements
    #[must_use]
    fn num_elements(&self) -> usize;

    /// Return the size of a given dimension within a larger space.
    ///
    /// Assumes the defined shape are the inner dimensions and will return 1
    /// when the specified dimension is out of the defined shape.
    ///
    /// For example, a shape of the form [2, 4, 5] with calls:
    ///  shape.get_dim(4, 0) will return 1 (dimension outside of defined shape)
    ///  shape.get_dim(4, 1) will return 2
    ///  shape.get_dim(4, 2) will return 4
    ///  shape.get_dim(4, 3) will return 5
    #[must_use]
    fn get_dim(&self, total_dims: usize, i: usize) -> usize;

    /// Get access to the underlying shape
    #[must_use]
    fn shape(&self) -> &Shape;
}

impl<T> HasShape for &T
where
    T: HasShape,
{
    fn num_dims(&self) -> usize {
        (*self).num_dims()
    }

    fn num_elements(&self) -> usize {
        (*self).num_elements()
    }

    fn get_dim(&self, total_dims: usize, i: usize) -> usize {
        (*self).get_dim(total_dims, i)
    }

    fn shape(&self) -> &Shape {
        (*self).shape()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Shape {
    dims: Vec<usize>,
}

impl Display for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", shape_string(&self.dims))
    }
}

impl Shape {
    pub fn new(dims: &[usize]) -> Result<Self, SimError> {
        if dims.contains(&0) {
            return Err(SimError(format!("Shape {dims:?} has zero elements")));
        }

        dims.iter().try_fold(1usize, |num_elements, dim| {
            num_elements
                .checked_mul(*dim)
                .ok_or_else(|| SimError(format!("Shape {dims:?} element count overflows")))
        })?;

        Ok(Self {
            dims: dims.to_vec(),
        })
    }

    #[must_use]
    pub fn get_dims(&self) -> &Vec<usize> {
        &self.dims
    }
}

impl HasShape for Shape {
    fn num_dims(&self) -> usize {
        self.dims.len()
    }

    fn num_elements(&self) -> usize {
        self.dims.iter().product()
    }

    fn get_dim(&self, total_dims: usize, i: usize) -> usize {
        let dim_index = total_dims - i;
        let rank = self.num_dims();
        if dim_index <= rank {
            self.dims[rank - dim_index]
        } else {
            1
        }
    }

    fn shape(&self) -> &Shape {
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Offsets(Vec<usize>);

impl Offsets {
    #[must_use]
    pub fn get_dims(&self) -> &Vec<usize> {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct Tensor {
    id: Option<String>,
    dtype: DataType,
    shape: Shape,
    addr: u64,
    num_bytes: usize,
}

impl Tensor {
    /// Create a tensor
    pub fn new(dims: &[usize], dtype: &DataType, addr: u64) -> Result<Self, SimError> {
        Self::from_shape(Shape::new(dims)?, dtype, addr)
    }

    /// Create a tensor from an already validated shape.
    pub fn from_shape(shape: Shape, dtype: &DataType, addr: u64) -> Result<Self, SimError> {
        let num_bytes = checked_num_bytes(shape.num_elements(), dtype, "Tensor")?;
        Ok(Self {
            id: None,
            shape,
            dtype: *dtype,
            addr,
            num_bytes,
        })
    }

    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn set_id(&mut self, id: impl Into<String>) {
        self.id = Some(id.into());
    }

    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Return the number of bytes this entire Tensor will consume in memory.
    ///
    /// This currently assumes it is optimally packed into memory bytes
    #[must_use]
    pub fn num_bytes(&self) -> usize {
        self.num_bytes
    }

    #[must_use]
    pub fn dtype(&self) -> &DataType {
        &self.dtype
    }

    #[must_use]
    pub fn addr(&self) -> u64 {
        self.addr
    }

    pub fn set_addr(&mut self, addr: u64) {
        self.addr = addr;
    }
}

impl HasShape for Tensor {
    fn num_dims(&self) -> usize {
        self.shape.num_dims()
    }

    fn num_elements(&self) -> usize {
        self.shape.num_elements()
    }

    fn get_dim(&self, total_dims: usize, i: usize) -> usize {
        self.shape.get_dim(total_dims, i)
    }

    fn shape(&self) -> &Shape {
        &self.shape
    }
}

/// A view into a tensor
#[derive(Clone, Debug)]
pub struct TensorView {
    tensor: Tensor,
    shape: Shape,
    offsets: Offsets,
    num_bytes: usize,
}

impl TensorView {
    /// Create a view into the given tensor
    pub fn new(tensor: Tensor, shape: &[usize], offsets: &[usize]) -> Result<Self, SimError> {
        let shape = Shape::new(shape)?;
        let tensor_dims = tensor.shape().get_dims();
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

        for ((offset, extent), tensor_extent) in
            offsets.iter().zip(shape.get_dims()).zip(tensor_dims)
        {
            let end = offset.checked_add(*extent).ok_or_else(|| {
                SimError(format!(
                    "Tensor view offset {offset} plus extent {extent} overflows"
                ))
            })?;
            if end > *tensor_extent {
                return sim_error!(
                    "Tensor view range {offset}..{end} is out of range for dimension of size {tensor_extent}"
                );
            }
        }

        let num_bytes = checked_num_bytes(shape.num_elements(), tensor.dtype(), "Tensor view")?;
        Ok(Self {
            tensor,
            shape,
            offsets: Offsets(offsets.to_vec()),
            num_bytes,
        })
    }

    /// Create a view which is the full tensor
    #[must_use]
    pub fn new_full(tensor: Tensor) -> Self {
        let shape = tensor.shape().clone();
        let offsets = Offsets(vec![0; tensor.num_dims()]);
        let num_bytes = tensor.num_bytes();
        Self {
            tensor,
            shape,
            offsets,
            num_bytes,
        }
    }

    #[must_use]
    pub fn tensor(&self) -> &Tensor {
        &self.tensor
    }

    #[must_use]
    pub fn offsets(&self) -> &Offsets {
        &self.offsets
    }

    #[must_use]
    pub fn is_full_view(&self) -> bool {
        self.shape == *self.tensor.shape() && self.offsets.0.iter().all(|offset| *offset == 0)
    }

    pub fn from_output_partition(
        tensor: Tensor,
        output_rank: usize,
        partition_dim: usize,
        partition_offset: usize,
        partition_len: usize,
    ) -> Result<Self, SimError> {
        Self::from_output_partitions(
            tensor,
            output_rank,
            &[DimPartition {
                dim: partition_dim,
                offset: partition_offset,
                len: partition_len,
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

    /// Create a view by applying output partitions to an existing base view.
    ///
    /// This preserves the base view's offsets and shape constraints.
    pub fn from_output_partitions_on_view(
        base_view: &TensorView,
        output_rank: usize,
        partitions: &[DimPartition],
    ) -> Result<Self, SimError> {
        let view_rank = base_view.num_dims();
        let rank_pad = output_rank.saturating_sub(view_rank);
        let mut shape = base_view.shape().get_dims().clone();
        let mut offsets = base_view.offsets().get_dims().clone();

        for partition in partitions {
            if partition.dim < rank_pad {
                continue;
            }

            let view_dim = partition.dim - rank_pad;
            if view_dim < view_rank && shape[view_dim] > 1 {
                let partition_end =
                    partition.offset.checked_add(partition.len).ok_or_else(|| {
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
                shape[view_dim] = partition.len;
            }
        }

        Self::new(base_view.tensor().clone(), &shape, &offsets)
    }

    #[must_use]
    pub fn num_bytes(&self) -> usize {
        self.num_bytes
    }

    /// Return the physical byte ranges touched by this view, relative to the
    /// tensor's base address.
    pub fn byte_ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        let tensor_dims = self.tensor.shape().get_dims();
        let view_dims = self.shape().get_dims();
        let offsets = self.offsets().get_dims();
        let rank = self.num_dims();

        // The innermost dimension is contiguous even when the view only covers
        // part of it. A preceding dimension can join the same run when every
        // enclosed inner dimension is covered in full from offset zero.
        let mut contiguous_start = rank.saturating_sub(1);
        while contiguous_start > 0
            && offsets[contiguous_start] == 0
            && view_dims[contiguous_start] == tensor_dims[contiguous_start]
        {
            contiguous_start -= 1;
        }

        let run_elements = view_dims[contiguous_start..].iter().product::<usize>();
        let outer_runs = view_dims[..contiguous_start].iter().product::<usize>();
        let bits_per_element = self.tensor.dtype().num_bits();
        let ranges = (0..outer_runs).scan(vec![0; contiguous_start], move |outer_coordinate, _| {
            let start_element = tensor_dims
                .iter()
                .enumerate()
                .fold(0usize, |flat, (dim, size)| {
                    let relative_coordinate = outer_coordinate.get(dim).copied().unwrap_or(0);
                    flat * size + offsets[dim] + relative_coordinate
                });
            let end_element = start_element + run_elements;

            for dim in (0..contiguous_start).rev() {
                outer_coordinate[dim] += 1;
                if outer_coordinate[dim] < view_dims[dim] {
                    break;
                }
                outer_coordinate[dim] = 0;
            }

            Some(element_range_to_byte_range(
                start_element..end_element,
                bits_per_element,
            ))
        });

        coalesce_ranges(ranges)
    }

    /// Return the physical byte range enclosing every byte touched by this
    /// view, relative to the tensor's base address.
    ///
    /// Unlike [`Self::byte_ranges`], these bounds may include untouched bytes
    /// between the separate physical ranges of a strided view.
    #[must_use]
    pub fn byte_bounds(&self) -> Range<usize> {
        let tensor_dims = self.tensor.shape().get_dims();
        let view_dims = self.shape().get_dims();
        let offsets = self.offsets().get_dims();
        let start_element = self.element_offset();
        let end_element = tensor_dims
            .iter()
            .enumerate()
            .fold(0usize, |flat, (dim, size)| {
                flat * size + offsets[dim] + view_dims[dim] - 1
            })
            + 1;

        element_range_to_byte_range(start_element..end_element, self.tensor.dtype().num_bits())
    }

    /// Return the total number of physical bytes touched by this view.
    #[must_use]
    pub fn num_access_bytes(&self) -> usize {
        self.byte_ranges().map(|range| range.len()).sum()
    }

    /// Return the offset of the first element (in number of elements).
    #[must_use]
    pub fn element_offset(&self) -> usize {
        self.tensor
            .shape()
            .get_dims()
            .iter()
            .zip(self.offsets().get_dims())
            .fold(0, |flat, (dim, offset)| flat * dim + offset)
    }
}

fn element_range_to_byte_range(
    element_range: Range<usize>,
    bits_per_element: usize,
) -> Range<usize> {
    const BYTE_OFFSET_INVARIANT: &str = "tensor construction guarantees byte offsets fit in usize";

    let start_bit = element_range.start as u128 * bits_per_element as u128;
    let end_bit = element_range.end as u128 * bits_per_element as u128;
    let start_byte = usize::try_from(start_bit / 8).expect(BYTE_OFFSET_INVARIANT);
    let end_byte = usize::try_from(end_bit.div_ceil(8)).expect(BYTE_OFFSET_INVARIANT);
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

fn checked_num_bytes(
    num_elements: usize,
    dtype: &DataType,
    description: &str,
) -> Result<usize, SimError> {
    let bits_per_element = dtype.num_bits();
    let complete_groups = num_elements / 8;
    let remaining_elements = num_elements % 8;

    complete_groups
        .checked_mul(bits_per_element)
        .and_then(|complete_bytes| {
            remaining_elements
                .checked_mul(bits_per_element)
                .and_then(|remaining_bits| complete_bytes.checked_add(remaining_bits.div_ceil(8)))
        })
        .ok_or_else(|| SimError(format!("{description} storage size overflows")))
}

impl HasShape for TensorView {
    fn num_dims(&self) -> usize {
        self.shape.num_dims()
    }

    fn num_elements(&self) -> usize {
        self.shape.num_elements()
    }

    fn get_dim(&self, total_dims: usize, i: usize) -> usize {
        self.shape.get_dim(total_dims, i)
    }

    fn shape(&self) -> &Shape {
        &self.shape
    }
}

#[derive(Clone, Debug)]
pub struct TensorPartition {
    pub inputs: Vec<Option<TensorView>>,
    pub outputs: Vec<Option<TensorView>>,
}

pub trait Operator {
    /// Validate that the input and output tensors are valid shapes and
    /// datatypes
    fn validate_tensors(&self, inputs: &[Option<Tensor>], outputs: &[Option<Tensor>]) -> SimResult;

    /// Returns the number of clock ticks needed to perform the
    /// specified computation give the machine capabilities
    fn compute_delay_ticks(
        &self,
        compute_capabilities: &Rc<ComputeCapabilities>,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError>;

    /// Returns the total number of FLOPs performed by the specified
    /// computation.
    fn compute_flops(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        Ok(self.compute_machine_ops(inputs, outputs)?.total())
    }

    /// Returns the number of machine operations performed by the specified
    /// computation, broken down by operation type.
    fn compute_machine_ops(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<MachineOpCounts, SimError>;

    /// Partition the operation into one or more views that can be executed in
    /// parallel. Implementations may return fewer than `num_partitions` if the
    /// operator cannot be split that finely.
    fn partition_views(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError>;
}

/// Create partitions from full Tensors
///
/// This is a wrapper function to create TensorViews and then call
/// `create_partitions` using those views.
pub fn partition_tensors<T: Operator>(
    operator: &T,
    input_tensors: &[Option<Tensor>],
    output_tensors: &[Option<Tensor>],
    num_partitions: usize,
) -> Result<Vec<TensorPartition>, SimError> {
    let input_views = input_tensors
        .iter()
        .map(|maybe_tensor| {
            maybe_tensor
                .as_ref()
                .map(|tensor| TensorView::new_full(tensor.clone()))
        })
        .collect::<Vec<_>>();
    let output_views = output_tensors
        .iter()
        .map(|maybe_tensor| {
            maybe_tensor
                .as_ref()
                .map(|tensor| TensorView::new_full(tensor.clone()))
        })
        .collect::<Vec<_>>();
    operator.partition_views(&input_views, &output_views, num_partitions)
}

fn partition_into_ranges(total: usize, requested: usize) -> Vec<(usize, usize)> {
    // Determine a valid number of partitions such that: total >= partitions >=1
    let partitions = requested.clamp(1, total.max(1));

    // All ranges get this number of entries
    let base_range_size = total / partitions;

    // This number of ranges get an extra 1
    let remainder = total % partitions;

    let mut start = 0;
    let mut ranges = Vec::with_capacity(partitions);

    for i in 0..partitions {
        let len = base_range_size + usize::from(i < remainder);
        if len == 0 {
            continue;
        }
        ranges.push((start, len));
        start += len;
    }

    if ranges.is_empty() {
        ranges.push((0, total.max(1)));
    }

    ranges
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimPartition {
    pub dim: usize,
    pub offset: usize,
    pub len: usize,
}

#[must_use]
pub fn partition_across_dimensions(
    dims: &[usize],
    candidate_dims: &[usize],
    requested: usize,
) -> Vec<Vec<DimPartition>> {
    let requested = requested.max(1);
    let mut split_dims = Vec::new();
    let mut achieved_partitions = 1usize;

    for &dim in candidate_dims {
        let dim_extent = dims[dim];
        if dim_extent <= 1 {
            continue;
        }

        let needed = requested.div_ceil(achieved_partitions).max(1);
        let splits = dim_extent.min(needed);
        if splits <= 1 {
            continue;
        }

        split_dims.push((dim, partition_into_ranges(dim_extent, splits)));
        achieved_partitions *= splits;
        if achieved_partitions >= requested {
            break;
        }
    }

    if split_dims.is_empty() {
        // In the case we are just requesting a single partition we just preserve the
        // shape
        let preserve_shape: Vec<DimPartition> = dims
            .iter()
            .enumerate()
            .map(|(idx, dim)| DimPartition {
                dim: idx,
                offset: 0,
                len: *dim,
            })
            .collect();
        return vec![preserve_shape];
    }

    let mut partitions = vec![Vec::new()];
    for (dim, ranges) in split_dims {
        let mut next = Vec::with_capacity(partitions.len() * ranges.len());
        for base in &partitions {
            for (offset, len) in &ranges {
                let mut partition = base.clone();
                partition.push(DimPartition {
                    dim,
                    offset: *offset,
                    len: *len,
                });
                next.push(partition);
            }
        }
        partitions = next;
    }

    partitions
}

#[must_use]
pub fn apply_dim_partitions(
    dims: &[usize],
    partitions: &[DimPartition],
) -> (Vec<usize>, Vec<usize>) {
    let mut shape = dims.to_vec();
    let mut offsets = vec![0; dims.len()];

    for partition in partitions {
        shape[partition.dim] = partition.len;
        offsets[partition.dim] = partition.offset;
    }

    (shape, offsets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_rejects_element_count_overflow() {
        assert!(Shape::new(&[usize::MAX, 2]).is_err());
    }

    #[test]
    fn tensor_rejects_storage_size_overflow() {
        assert!(Tensor::new(&[usize::MAX / 2], &DataType::Int64, 0).is_err());
    }

    #[test]
    fn tensor_accepts_representable_byte_count_without_bit_overflow() {
        let int8 = Tensor::new(&[usize::MAX], &DataType::Int8, 0).unwrap();
        assert_eq!(int8.num_bytes(), usize::MAX);

        let int4 = Tensor::new(&[usize::MAX], &DataType::Int4, 0).unwrap();
        assert_eq!(int4.num_bytes(), usize::MAX.div_ceil(2));
    }

    #[test]
    fn scalar_shapes_are_preserved() {
        let scalar = Shape::new(&[]).unwrap();
        assert_eq!(scalar.num_elements(), 1);

        let scalar_tensor = Tensor::new(&[], &DataType::Fp32, 0).unwrap();
        assert_eq!(scalar_tensor.num_elements(), 1);
        assert_eq!(scalar_tensor.num_bytes(), 4);
    }

    #[test]
    fn zero_element_shapes_are_rejected() {
        assert!(Shape::new(&[usize::MAX, 2, 0]).is_err());
        assert!(Tensor::new(&[usize::MAX, 2, 0], &DataType::Fp32, 0).is_err());
    }

    #[test]
    fn tensor_view_rejects_mismatched_ranks() {
        let tensor = Tensor::new(&[4, 4], &DataType::Fp32, 0).unwrap();

        assert!(TensorView::new(tensor.clone(), &[1], &[0, 0]).is_err());
        assert!(TensorView::new(tensor, &[1, 1], &[0]).is_err());
    }

    #[test]
    fn tensor_view_rejects_overflowing_and_out_of_range_extents() {
        let tensor = Tensor::new(&[4], &DataType::Fp32, 0).unwrap();

        assert!(TensorView::new(tensor.clone(), &[1], &[usize::MAX]).is_err());
        assert!(TensorView::new(tensor, &[3], &[2]).is_err());
    }

    #[test]
    fn tensor_view_rejects_zero_extent() {
        let tensor = Tensor::new(&[4], &DataType::Fp32, 0).unwrap();
        assert!(TensorView::new(tensor, &[0], &[4]).is_err());
    }

    #[test]
    fn tensor_view_partition_rejects_overflow_and_base_view_escape() {
        let tensor = Tensor::new(&[8], &DataType::Fp32, 0).unwrap();
        let view = TensorView::new(tensor, &[4], &[2]).unwrap();

        assert!(
            TensorView::from_output_partitions_on_view(
                &view,
                1,
                &[DimPartition {
                    dim: 0,
                    offset: usize::MAX,
                    len: 1,
                }],
            )
            .is_err()
        );
        assert!(
            TensorView::from_output_partitions_on_view(
                &view,
                1,
                &[DimPartition {
                    dim: 0,
                    offset: 4,
                    len: 1,
                }],
            )
            .is_err()
        );
    }

    #[test]
    fn strided_packed_view_returns_exact_byte_ranges() {
        let tensor = Tensor::new(&[4, 4], &DataType::Int4, 0).unwrap();
        let view = TensorView::new(tensor, &[3, 1], &[1, 1]).unwrap();

        assert_eq!(
            view.byte_ranges().collect::<Vec<_>>(),
            vec![2..3, 4..5, 6..7]
        );
        assert_eq!(view.byte_bounds(), 2..7);
        assert_eq!(view.num_access_bytes(), 3);
    }

    #[test]
    fn physically_contiguous_multidimensional_view_returns_one_range() {
        let tensor = Tensor::new(&[4, 4], &DataType::Int8, 0).unwrap();
        let view = TensorView::new(tensor, &[2, 4], &[1, 0]).unwrap();

        assert_eq!(view.byte_ranges().collect::<Vec<_>>(), vec![4..12]);
        assert_eq!(view.byte_bounds(), 4..12);
        assert_eq!(view.num_access_bytes(), 8);
    }

    #[test]
    fn packed_byte_ranges_are_coalesced_when_they_overlap_or_touch() {
        let ranges = coalesce_ranges([1..3, 2..4, 4..5].into_iter()).collect::<Vec<_>>();
        assert_eq!(ranges, vec![1..5]);

        let tensor = Tensor::new(&[2, 3], &DataType::Int4, 0).unwrap();
        let view = TensorView::new(tensor, &[2, 1], &[0, 0]).unwrap();
        assert_eq!(view.byte_ranges().collect::<Vec<_>>(), vec![0..2]);
    }

    #[test]
    fn byte_ranges_accept_representable_size_without_bit_overflow() {
        let int8 = TensorView::new_full(Tensor::new(&[usize::MAX], &DataType::Int8, 0).unwrap());
        assert_eq!(int8.byte_ranges().collect::<Vec<_>>(), vec![0..usize::MAX]);

        let int4 = TensorView::new_full(Tensor::new(&[usize::MAX], &DataType::Int4, 0).unwrap());
        assert_eq!(
            int4.byte_ranges().collect::<Vec<_>>(),
            vec![0..usize::MAX.div_ceil(2)]
        );
    }

    #[test]
    fn large_strided_view_byte_ranges_are_generated_lazily() {
        let tensor = Tensor::new(&[100_000_000, 2], &DataType::Int8, 0).unwrap();
        let view = TensorView::new(tensor, &[100_000_000, 1], &[0, 0]).unwrap();

        assert_eq!(
            view.byte_ranges().take(3).collect::<Vec<_>>(),
            vec![0..1, 2..3, 4..5]
        );
    }
}

pub mod add;
pub mod custom;
pub mod gemm;
pub mod maxpool;
