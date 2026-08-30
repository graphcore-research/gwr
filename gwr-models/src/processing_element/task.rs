// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::types::SimError;
use rand::RngExt;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::processing_element::operators::{
    ExpansionDirection, HasShape, Operator, OperatorAdd, OperatorCustom, OperatorGemm,
    OperatorMaxPool, Shape, Tensor, TensorPartition, TensorView, create_maxpool_op, gemm_rhs_shape,
    maybe_add_indices_output, maybe_add_input_c, partition_tensors,
};
use crate::processing_element::{ComputeCapabilities, MachineOpCounts};

#[derive(Debug, Clone)]
pub struct ComputeTaskConfig {
    /// Only needed as a debug aid
    pub id: String,
    pub op: ComputeOp,
    pub inputs: Vec<Option<TensorView>>,
    pub outputs: Vec<Option<TensorView>>,
}

impl ComputeTaskConfig {
    #[must_use]
    pub fn activity_name(&self) -> &str {
        match &self.op {
            ComputeOp::Custom(operator) => operator.name.as_deref().unwrap_or(&self.id),
            _ => &self.id,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputeOp {
    Add,
    Gemm,
    MaxPool(OperatorMaxPool),
    Custom(OperatorCustom),
}

impl Serialize for ComputeOp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Add => serializer.serialize_str("add"),
            Self::Gemm => serializer.serialize_str("gemm"),
            Self::MaxPool(operator) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("maxpool", operator)?;
                map.end()
            }
            Self::Custom(operator) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("custom", operator)?;
                map.end()
            }
        }
    }
}

impl ComputeOp {
    #[must_use]
    pub fn trace_name(&self) -> &str {
        match self {
            ComputeOp::Add => "add",
            ComputeOp::Gemm => "gemm",
            ComputeOp::MaxPool(_) => "maxpool",
            ComputeOp::Custom(operator) => operator.name.as_deref().unwrap_or("custom"),
        }
    }

    pub fn compute_delay_ticks(
        &self,
        compute_capabilities: &Rc<ComputeCapabilities>,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        self.operator()
            .compute_delay_ticks(compute_capabilities, input_views, output_views)
    }

    pub fn validate(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
    ) -> Result<(), SimError> {
        self.operator().validate(input_views, output_views)
    }

    pub fn compute_machine_ops(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
    ) -> Result<MachineOpCounts, SimError> {
        self.operator()
            .compute_machine_ops(input_views, output_views)
    }

    pub fn create_partitions(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        self.operator()
            .partition_views(input_views, output_views, num_partitions)
    }

    pub fn create_tensor_partitions(
        &self,
        inputs: &[Option<Tensor>],
        outputs: &[Option<Tensor>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        partition_tensors(self.operator(), inputs, outputs, num_partitions)
    }

    pub fn create_inputs(
        &self,
        outputs: &[Option<Tensor>],
        expand_ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<Vec<Option<Tensor>>, SimError> {
        match self {
            Self::Add => OperatorAdd::create_inputs(outputs, expand_ratio, rng),
            Self::Gemm => OperatorGemm::create_inputs(outputs, expand_ratio, rng),
            Self::MaxPool(operator) => operator.create_inputs(outputs, expand_ratio, rng),
            Self::Custom(_) => Err(SimError(
                "Custom operators cannot generate tensors".to_string(),
            )),
        }
    }

    pub fn create_outputs(
        &self,
        inputs: &[Option<Tensor>],
        expand_ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<Vec<Option<Tensor>>, SimError> {
        match self {
            Self::Add => OperatorAdd::create_outputs(inputs, expand_ratio, rng),
            Self::Gemm => OperatorGemm::create_outputs(inputs, expand_ratio, rng),
            Self::MaxPool(operator) => operator.create_outputs(inputs, expand_ratio, rng),
            Self::Custom(_) => Err(SimError(
                "Custom operators cannot generate tensors".to_string(),
            )),
        }
    }

    pub fn configured_for_tensor<T: HasShape>(
        &self,
        tensor: &T,
        direction: ExpansionDirection,
        expand_ratio: f64,
    ) -> Result<Self, SimError> {
        match self {
            Self::Add => Ok(Self::Add),
            Self::Gemm => Ok(Self::Gemm),
            Self::MaxPool(_) => Ok(Self::MaxPool(create_maxpool_op(
                tensor,
                direction,
                expand_ratio,
            )?)),
            Self::Custom(_) => Err(SimError("Custom operators cannot be generated".to_string())),
        }
    }

    pub fn add_optional_inputs(
        &self,
        inputs: &mut Vec<Option<Tensor>>,
        expand_ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<bool, SimError> {
        match self {
            Self::Gemm => maybe_add_input_c(inputs, expand_ratio, rng),
            _ => Ok(false),
        }
    }

    pub fn add_optional_outputs(
        &self,
        outputs: &mut Vec<Option<Tensor>>,
        expand_ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<bool, SimError> {
        match self {
            Self::MaxPool(_) => maybe_add_indices_output(outputs, expand_ratio, rng),
            _ => Ok(false),
        }
    }

    pub fn gemm_rhs_shape<T: HasShape>(input: &T) -> Result<Shape, SimError> {
        gemm_rhs_shape(input)
    }

    fn operator(&self) -> &dyn Operator {
        static ADD: OperatorAdd = OperatorAdd {};
        static GEMM: OperatorGemm = OperatorGemm {};

        match self {
            Self::Add => &ADD,
            Self::Gemm => &GEMM,
            Self::MaxPool(operator) => operator,
            Self::Custom(operator) => operator,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SyncRegion {
    Local,
    Global,
}

#[derive(Debug, Clone)]
pub enum Task {
    ComputeTask { config: ComputeTaskConfig },
    SyncTask { region: SyncRegion },
}
