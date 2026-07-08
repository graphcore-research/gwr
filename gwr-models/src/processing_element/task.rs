// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::types::SimError;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::processing_element::ComputeCapabilities;
use crate::processing_element::operators::add::OperatorAdd;
use crate::processing_element::operators::gemm::OperatorGemm;
use crate::processing_element::operators::maxpool::OperatorMaxPool;
use crate::processing_element::operators::{Operator, TensorPartition, TensorView};

#[derive(Debug, Clone)]
pub struct ComputeTaskConfig {
    /// Only needed as a debug aid
    pub id: String,
    pub op: ComputeOp,
    pub inputs: Vec<Option<TensorView>>,
    pub outputs: Vec<Option<TensorView>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputeOp {
    Add,
    Gemm,
    MaxPool(OperatorMaxPool),
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
        }
    }
}

impl ComputeOp {
    #[must_use]
    pub fn trace_name(&self) -> &'static str {
        match self {
            ComputeOp::Add => "add",
            ComputeOp::Gemm => "gemm",
            ComputeOp::MaxPool(_) => "maxpool",
        }
    }

    pub fn compute_delay_ticks(
        &self,
        compute_capabilities: &Rc<ComputeCapabilities>,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        match self {
            ComputeOp::Add => {
                OperatorAdd {}.compute_delay_ticks(compute_capabilities, input_views, output_views)
            }
            ComputeOp::Gemm => {
                OperatorGemm {}.compute_delay_ticks(compute_capabilities, input_views, output_views)
            }
            ComputeOp::MaxPool(operator) => {
                operator.compute_delay_ticks(compute_capabilities, input_views, output_views)
            }
        }
    }

    pub fn compute_flops(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        match self {
            ComputeOp::Add => OperatorAdd {}.compute_flops(input_views, output_views),
            ComputeOp::Gemm => OperatorGemm {}.compute_flops(input_views, output_views),
            ComputeOp::MaxPool(operator) => operator.compute_flops(input_views, output_views),
        }
    }

    pub fn create_partitions(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        match self {
            ComputeOp::Add => {
                OperatorAdd {}.partition_views(input_views, output_views, num_partitions)
            }
            ComputeOp::Gemm => {
                OperatorGemm {}.partition_views(input_views, output_views, num_partitions)
            }
            ComputeOp::MaxPool(operator) => {
                operator.partition_views(input_views, output_views, num_partitions)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryTaskConfig {
    /// Only needed as a debug aid
    pub id: String,
    pub op: MemoryOp,
    pub addr: u64,
    pub num_bytes: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryOp {
    Load,
    Store,
}

#[derive(Debug, Clone, Copy)]
pub enum SyncRegion {
    Local,
    Global,
}

#[derive(Debug, Clone)]
pub enum Task {
    ComputeTask { config: ComputeTaskConfig },
    MemoryTask { config: MemoryTaskConfig },
    SyncTask { region: SyncRegion },
}
