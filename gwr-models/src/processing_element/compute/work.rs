// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#[cfg(test)]
use std::collections::BTreeMap;

use gwr_engine::types::SimError;
use serde::{Deserialize, Serialize};

use super::ExecutionUnitId;
use crate::processing_element::MachineOpCounts;
use crate::processing_element::tensors::dtype::DataType;

/// Sequential stages of named operations before selecting execution units.
#[derive(Clone, Debug)]
pub struct ComputeWork {
    pub(in crate::processing_element) stages: Vec<WorkStage>,
}

impl ComputeWork {
    /// Matrix work with checked logical geometry; service shape is selected per
    /// candidate.
    pub fn matrix_mac(
        inputs: Vec<DataType>,
        accumulator: DataType,
        shape: MatrixWorkShape,
    ) -> Result<Self, SimError> {
        if inputs.len() != 2 {
            return Err(SimError("matrix shape requires two operands".into()));
        }
        let count = shape.logical_macs()?;
        Ok(Self {
            stages: vec![WorkStage {
                operations: vec![TypedWork {
                    signature: CapabilityKey::MatrixMac {
                        inputs,
                        accumulator,
                    },
                    count,
                    matrix_shape: Some(shape),
                }],
            }],
        })
    }

    #[must_use]
    pub fn operation(op: impl Into<String>, inputs: Vec<DataType>, count: usize) -> Self {
        Self {
            stages: vec![WorkStage {
                operations: vec![TypedWork::operation(op, inputs, count)],
            }],
        }
    }

    /// Combine corresponding stages, retaining barriers between stage indices.
    #[cfg(test)]
    #[must_use]
    pub(in crate::processing_element) fn parallel(works: Vec<Self>) -> Self {
        let mut stages: Vec<WorkStage> = Vec::new();
        for work in works {
            for (index, stage) in work.stages.into_iter().enumerate() {
                if index == stages.len() {
                    stages.push(stage);
                } else {
                    stages[index].operations.extend(stage.operations);
                }
            }
        }
        Self { stages }
    }
}

/// Independent work; a barrier separates consecutive stages.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::processing_element) struct WorkStage {
    /// Independent typed demands, grouped into compute batches during lowering.
    pub(in crate::processing_element) operations: Vec<TypedWork>,
}

/// A number of operations of one exact signature.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::processing_element) struct TypedWork {
    /// Exact capability required.
    pub(in crate::processing_element) signature: CapabilityKey,
    /// Logical operation count.
    pub(in crate::processing_element) count: usize,
    /// Logical batch count, output tile dimensions and contraction chunk
    /// length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::processing_element) matrix_shape: Option<MatrixWorkShape>,
}

impl TypedWork {
    #[must_use]
    pub(in crate::processing_element) fn operation(
        op: impl Into<String>,
        inputs: Vec<DataType>,
        count: usize,
    ) -> Self {
        Self {
            signature: CapabilityKey::Operation {
                op: op.into(),
                inputs,
            },
            count,
            matrix_shape: None,
        }
    }

    pub(crate) fn machine_ops(&self, flops_per_op: usize) -> Result<MachineOpCounts, SimError> {
        MachineOpCounts::single(self.signature.op(), self.count, flops_per_op)
    }
}

/// Logical geometry for independent matrix contractions in one compute batch.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixWorkShape {
    /// Independent contractions; shared or repeated output axes form batches.
    pub batches: usize,
    pub m: usize,
    pub n: usize,
    pub k: usize,
}

impl MatrixWorkShape {
    /// Checked logical MACs; zero dimensions are malformed, not empty work.
    pub(in crate::processing_element) fn logical_macs(self) -> Result<usize, SimError> {
        if self.batches == 0 || self.m == 0 || self.n == 0 || self.k == 0 {
            return Err(SimError("matrix shape dimensions must be positive".into()));
        }
        self.batches
            .checked_mul(self.m)
            .and_then(|bm| bm.checked_mul(self.n))
            .and_then(|mn| mn.checked_mul(self.k))
            .ok_or_else(|| SimError("matrix shape MAC count overflows".into()))
    }
}

/// An exact typed operation, used for runtime matching (never contains sets).
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::processing_element) enum CapabilityKey {
    Operation {
        op: String,
        inputs: Vec<DataType>,
    },
    MatrixMac {
        inputs: Vec<DataType>,
        accumulator: DataType,
    },
}

impl CapabilityKey {
    #[must_use]
    pub(in crate::processing_element) fn op(&self) -> &str {
        match self {
            Self::Operation { op, .. } => op,
            Self::MatrixMac { .. } => "matrix_mac",
        }
    }

    /// Ordered arithmetic operands.
    #[must_use]
    pub(in crate::processing_element) fn inputs(&self) -> &[DataType] {
        match self {
            Self::Operation { inputs, .. } | Self::MatrixMac { inputs, .. } => inputs,
        }
    }
}

/// Sequential barriers containing independent compute batches.
#[derive(Clone, Debug)]
pub(in crate::processing_element) struct ExecutionPlan {
    /// Batches in stable execution unit order, grouped into sequential stages.
    pub(in crate::processing_element) stages: Vec<Vec<ComputeBatch>>,
}

#[cfg(test)]
impl ExecutionPlan {
    /// Uncontended service time; runtime scheduling adds contention.
    pub(in crate::processing_element) fn service_ticks(&self) -> Result<u64, SimError> {
        self.stages.iter().try_fold(0_u64, |sum, stage| {
            sum.checked_add(stage_service_ticks(stage)?)
                .ok_or_else(|| SimError("compute duration overflows".into()))
        })
    }
}

#[cfg(test)]
fn stage_service_ticks(stage: &[ComputeBatch]) -> Result<u64, SimError> {
    let mut occupied = BTreeMap::new();
    for batch in stage {
        let (execution_unit, finish) = batch.earliest_finish(&occupied)?;
        occupied.insert(execution_unit, finish);
    }
    Ok(occupied.values().copied().max().unwrap_or(0))
}

/// Nonpreemptive work occupying one execution unit.
#[derive(Clone, Debug)]
pub(in crate::processing_element) struct ComputeBatch {
    /// Checked logical accumulator demand, independent of charged service
    /// work and program SRAM. Zero when no matching capacity is enabled.
    pub(in crate::processing_element) matrix_accumulator_bytes: u64,
    /// Eligible execution units and their service times, fastest first.
    pub(in crate::processing_element) alternatives: Vec<(ExecutionUnitId, u64)>,
    /// Mathematical contributions for monitoring and statistics.
    pub(in crate::processing_element) machine_ops: MachineOpCounts,
}

#[cfg(test)]
impl ComputeBatch {
    fn earliest_finish(
        &self,
        occupied: &BTreeMap<ExecutionUnitId, u64>,
    ) -> Result<(ExecutionUnitId, u64), SimError> {
        let mut earliest: Option<(u64, ExecutionUnitId)> = None;
        for (execution_unit, ticks) in &self.alternatives {
            let finish = occupied
                .get(execution_unit)
                .copied()
                .unwrap_or(0)
                .checked_add(*ticks)
                .ok_or_else(|| SimError("compute duration overflows".into()))?;
            let candidate = (finish, *execution_unit);
            if earliest.is_none_or(|previous| candidate < previous) {
                earliest = Some(candidate);
            }
        }
        earliest
            .map(|(finish, unit)| (unit, finish))
            .ok_or_else(|| SimError("compute batch has no execution units".into()))
    }
}
