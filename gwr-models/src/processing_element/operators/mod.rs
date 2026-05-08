// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! The Operators define what operations a Processing Element can perform

use std::fmt::Display;
use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};

use crate::processing_element::ComputeCapabilities;
use crate::processing_element::operators::dtype::DataType;

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

#[derive(Clone, Debug, PartialEq)]
pub struct Shape(Vec<usize>);

impl Display for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", shape_string(&self.0))
    }
}

impl Shape {
    #[must_use]
    pub fn new(dims: &[usize]) -> Self {
        Self(dims.to_vec())
    }

    #[must_use]
    pub fn get_dims(&self) -> &Vec<usize> {
        &self.0
    }
}

impl HasShape for Shape {
    fn num_dims(&self) -> usize {
        self.0.len()
    }

    fn num_elements(&self) -> usize {
        self.0.iter().product()
    }

    fn get_dim(&self, total_dims: usize, i: usize) -> usize {
        let dim_index = total_dims - i;
        let rank = self.num_dims();
        if dim_index <= rank {
            self.0[rank - dim_index]
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
}

impl Tensor {
    /// Create a tensor
    #[must_use]
    pub fn new(dims: &[usize], dtype: &DataType, addr: u64) -> Self {
        Self {
            id: None,
            shape: Shape(dims.to_vec()),
            dtype: *dtype,
            addr,
        }
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
        (self.num_elements() * self.dtype.num_bits()).div_ceil(8)
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
}

impl TensorView {
    /// Create a view into the given tensor
    #[must_use]
    pub fn new(tensor: Tensor, shape: &[usize], offsets: &[usize]) -> Self {
        Self {
            tensor,
            shape: Shape(shape.to_vec()),
            offsets: Offsets(offsets.to_vec()),
        }
    }

    /// Create a view which is the full tensor
    #[must_use]
    pub fn new_full(tensor: Tensor) -> Self {
        let shape = Shape(tensor.shape().get_dims().to_vec());
        let offsets = Offsets(vec![0; tensor.num_dims()]);
        Self {
            tensor,
            shape,
            offsets,
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

    #[must_use]
    pub fn from_output_partition(
        tensor: Tensor,
        output_rank: usize,
        partition_dim: usize,
        partition_offset: usize,
        partition_len: usize,
    ) -> Self {
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

    #[must_use]
    pub fn from_output_partitions(
        tensor: Tensor,
        output_rank: usize,
        partitions: &[DimPartition],
    ) -> Self {
        let base_view = Self::new_full(tensor);
        Self::from_output_partitions_on_view(&base_view, output_rank, partitions)
    }

    /// Create a view by applying output partitions to an existing base view.
    ///
    /// This preserves the base view's offsets and shape constraints.
    #[must_use]
    pub fn from_output_partitions_on_view(
        base_view: &TensorView,
        output_rank: usize,
        partitions: &[DimPartition],
    ) -> Self {
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
                offsets[view_dim] += partition.offset;
                shape[view_dim] = partition.len;
            }
        }

        Self::new(base_view.tensor().clone(), &shape, &offsets)
    }

    #[must_use]
    pub fn num_bytes(&self) -> usize {
        let dtype = self.tensor.dtype();
        let num_bits = dtype.num_bits();
        let num_elements = self.num_elements();
        (num_bits * num_elements).div_ceil(8)
    }

    /// Return the offset of the first element (in number of elements)
    pub fn element_offset(&self) -> Result<usize, SimError> {
        let shape = &self.tensor.shape.0;
        let offsets = &self.offsets.0;
        if shape.len() != offsets.len() {
            return sim_error!(
                "shape rank {} does not match offset rank {}",
                shape.len(),
                offsets.len()
            );
        }

        let mut stride = 1;
        let mut total = 0;
        for (dim, offset) in shape.iter().rev().zip(offsets.iter().rev()) {
            if offset >= dim {
                return sim_error!("offset {offset} is out of range for dimension of size {dim}");
            }
            total += offset * stride;
            stride *= *dim;
        }
        Ok(total)
    }
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
    ) -> Result<usize, SimError>;

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

pub mod add;
pub mod gemm;
pub mod maxpool;
