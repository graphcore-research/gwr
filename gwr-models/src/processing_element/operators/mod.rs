// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Tensor types and operations supported by processing elements.

use std::rc::Rc;

use gwr_engine::types::{SimError, SimResult};

use crate::processing_element::{ComputeCapabilities, MachineOpCounts};

pub mod add;
pub mod custom;
pub mod dtype;
pub mod gemm;
pub mod maxpool;
mod partition;
mod tensor;
mod tensor_view;

pub use add::OperatorAdd;
pub use custom::OperatorCustom;
pub use gemm::{OperatorGemm, gemm_rhs_shape, maybe_add_input_c};
pub use maxpool::{OperatorMaxPool, create_maxpool_op, maybe_add_indices_output};
pub(crate) use partition::partition_across_dimensions;
pub use partition::{DimPartition, TensorPartition};
pub use tensor::{HasShape, Shape, Tensor, shape_string};
pub use tensor_view::{TensorView, TensorViewLayout, TensorViewStride};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpansionDirection {
    Backward,
    Forward,
}

pub trait Operator {
    fn validate_tensors(&self, inputs: &[Option<Tensor>], outputs: &[Option<Tensor>]) -> SimResult;

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

    fn compute_flops(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        Ok(self.compute_machine_ops(inputs, outputs)?.total())
    }

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

pub fn partition_tensors<T: Operator>(
    operator: &T,
    inputs: &[Option<Tensor>],
    outputs: &[Option<Tensor>],
    num_partitions: usize,
) -> Result<Vec<TensorPartition>, SimError> {
    operator.partition_views(&full_views(inputs), &full_views(outputs), num_partitions)
}
