// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Tensor types and operations supported by processing elements.

use std::rc::Rc;

use gwr_engine::types::{SimError, SimResult};

use crate::processing_element::{ComputeCapabilities, MachineOpCounts};

mod add;
mod custom;
pub mod dtype;
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
pub(crate) use partition::partition_across_dimensions;
pub use partition::{DimPartition, TensorPartition};
pub use tensor::{HasShape, Shape, Tensor, shape_string};
pub use tensor_view::{TensorView, TensorViewLayout, TensorViewStride};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpansionDirection {
    Backward,
    Forward,
}

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

    fn partition_views(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError>;
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
    operator.partition_views(&full_views(inputs), &full_views(outputs), num_partitions)
}
