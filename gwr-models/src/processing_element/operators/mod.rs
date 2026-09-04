// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Tensor types and operations supported by processing elements.

use std::rc::Rc;

use gwr_engine::types::{SimError, SimResult};

use crate::processing_element::{ComputeCapabilities, MachineOpCounts};

mod add;
mod custom;
pub mod dtype;
mod floor_sum;
mod gemm;
mod maxpool;
mod partition;
mod tensor;
mod tensor_view;

pub(crate) use add::OperatorAdd;
pub use custom::OperatorCustom;
pub(crate) use gemm::{OperatorGemm, gemm_rhs_shape, maybe_add_input_c};
pub use maxpool::{AutoPad, OperatorMaxPool};
pub(crate) use maxpool::{create_maxpool_op, maybe_add_indices_output};
pub use partition::{DimPartition, TensorPartition};
pub(crate) use partition::{max_partitions_across_dimensions, partition_across_dimensions};
pub use tensor::{HasShape, Shape, Tensor, shape_string};
pub use tensor_view::{TensorView, TensorViewLayout, TensorViewStride};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpansionDirection {
    Backward,
    Forward,
}

pub(crate) type TensorPartitions<'a> =
    Box<dyn Iterator<Item = Result<TensorPartition, SimError>> + 'a>;

pub(crate) trait Operator {
    fn validate(&self, inputs: &[Option<TensorView>], outputs: &[Option<TensorView>]) -> SimResult;

    #[cfg(test)]
    fn validate_tensors(&self, inputs: &[Option<Tensor>], outputs: &[Option<Tensor>]) -> SimResult {
        self.validate(&full_views(inputs), &full_views(outputs))
    }

    fn compute_delay_ticks(
        &self,
        compute_capabilities: &Rc<ComputeCapabilities>,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError>;

    fn compute_machine_ops(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<MachineOpCounts, SimError>;

    /// Return the greatest useful partition count for these views.
    ///
    /// Implementations must ensure that the largest partition working set does
    /// not increase as the requested partition count increases up to this
    /// value.
    fn max_partition_count(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError>;

    fn partition_views<'a>(
        &'a self,
        inputs: &'a [Option<TensorView>],
        outputs: &'a [Option<TensorView>],
        num_partitions: usize,
    ) -> Result<TensorPartitions<'a>, SimError>;
}

pub(crate) fn full_views(tensors: &[Option<Tensor>]) -> Vec<Option<TensorView>> {
    tensors
        .iter()
        .map(|tensor| tensor.clone().map(TensorView::new_full))
        .collect()
}

pub(crate) fn partition_tensors(
    operator: &dyn Operator,
    inputs: &[Option<Tensor>],
    outputs: &[Option<Tensor>],
    num_partitions: usize,
) -> Result<Vec<TensorPartition>, SimError> {
    let input_views = full_views(inputs);
    let output_views = full_views(outputs);
    operator
        .partition_views(&input_views, &output_views, num_partitions)?
        .collect()
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::dtype::DataType;
    use super::{Shape, Tensor, TensorView};

    pub(crate) fn test_shape(dims: &[usize]) -> Shape {
        Shape::new(dims).unwrap()
    }

    pub(crate) fn test_tensor(dims: &[usize]) -> Option<Tensor> {
        Some(Tensor::new(dims, &DataType::Bf16, 0).unwrap())
    }

    pub(crate) fn test_tensor_view(dims: &[usize]) -> Option<TensorView> {
        test_tensor(dims).map(TensorView::new_full)
    }
}
