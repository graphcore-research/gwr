// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#[cfg(test)]
use std::collections::BTreeMap;

use gwr_engine::sim_error;
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
    /// candidate. Rejects zero dimensions or an overflowing logical MAC count.
    pub fn matrix_mac(
        inputs: [DataType; 2],
        accumulator: DataType,
        shape: MatrixWorkShape,
    ) -> Result<Self, SimError> {
        let shape = CheckedMatrixShape::new(shape)?;
        Ok(Self {
            stages: vec![WorkStage {
                operations: vec![TypedWork::MatrixMac {
                    inputs,
                    accumulator,
                    shape,
                }],
            }],
        })
    }

    /// Scalar work of one exact operand signature. A zero count needs no
    /// service and is valid even when the model does not support the operation.
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
#[derive(Clone, Debug)]
pub(in crate::processing_element) struct WorkStage {
    pub(in crate::processing_element) operations: Vec<TypedWork>,
}

/// Scalar demands carry a count; matrix demands derive it from checked
/// geometry.
#[derive(Clone, Debug)]
pub(in crate::processing_element) enum TypedWork {
    Operation {
        op: String,
        inputs: Vec<DataType>,
        count: usize,
    },
    MatrixMac {
        inputs: [DataType; 2],
        accumulator: DataType,
        shape: CheckedMatrixShape,
    },
}

impl TypedWork {
    #[must_use]
    pub(in crate::processing_element) fn operation(
        op: impl Into<String>,
        inputs: Vec<DataType>,
        count: usize,
    ) -> Self {
        Self::Operation {
            op: op.into(),
            inputs,
            count,
        }
    }

    pub(crate) fn machine_ops(&self, flops_per_op: usize) -> Result<MachineOpCounts, SimError> {
        MachineOpCounts::single(self.op(), self.count(), flops_per_op)
    }

    pub(in crate::processing_element) fn count(&self) -> usize {
        match self {
            Self::Operation { count, .. } => *count,
            Self::MatrixMac { shape, .. } => shape.count,
        }
    }

    pub(in crate::processing_element) fn op(&self) -> &str {
        match self {
            Self::Operation { op, .. } => op,
            Self::MatrixMac { .. } => "matrix_mac",
        }
    }

    pub(in crate::processing_element) fn inputs(&self) -> &[DataType] {
        match self {
            Self::Operation { inputs, .. } => inputs,
            Self::MatrixMac { inputs, .. } => inputs,
        }
    }
}

/// Only construction in this module can pair geometry with its checked count.
#[derive(Clone, Copy, Debug)]
pub(in crate::processing_element) struct CheckedMatrixShape {
    geometry: MatrixWorkShape,
    count: usize,
}

impl CheckedMatrixShape {
    fn new(geometry: MatrixWorkShape) -> Result<Self, SimError> {
        Ok(Self {
            count: geometry.logical_macs()?,
            geometry,
        })
    }

    pub(in crate::processing_element) fn geometry(self) -> MatrixWorkShape {
        self.geometry
    }
}

/// Logical geometry for independent matrix contractions in one compute batch.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixWorkShape {
    /// Independent contractions; shared or repeated output axes form batches.
    pub batches: usize,
    /// Logical output rows contributed by axes exclusive to the left operand.
    pub m: usize,
    /// Logical output columns contributed by axes exclusive to the right
    /// operand.
    pub n: usize,
    /// Length of the shared contraction axis in this work chunk.
    pub k: usize,
}

impl MatrixWorkShape {
    /// Checked logical MACs; zero dimensions are malformed, not empty work.
    pub(in crate::processing_element) fn logical_macs(self) -> Result<usize, SimError> {
        if self.batches == 0 || self.m == 0 || self.n == 0 || self.k == 0 {
            return sim_error!("matrix shape dimensions must be positive");
        }
        self.batches
            .checked_mul(self.m)
            .and_then(|bm| bm.checked_mul(self.n))
            .and_then(|mn| mn.checked_mul(self.k))
            .ok_or_else(|| SimError("matrix shape MAC count overflows".into()))
    }
}

/// Sequential barriers containing independent compute batches.
#[derive(Clone, Debug)]
pub(in crate::processing_element) struct ExecutionPlan {
    /// Batches in stable execution unit order, grouped into sequential stages.
    pub(in crate::processing_element) stages: Vec<Vec<ComputeBatch>>,
}

impl ExecutionPlan {
    /// A duration lower bound in ticks for one invocation. Batches may run in
    /// parallel, so each stage needs at least its longest minimum eligible
    /// service. Mandatory stage barriers add those bounds. Shared-unit
    /// contention and elapsed start times still require runtime checks.
    pub(in crate::processing_element) fn duration_lower_bound_ticks(
        &self,
    ) -> Result<u64, SimError> {
        self.stages.iter().try_fold(0_u64, |sum, stage| {
            let ticks = stage
                .iter()
                .filter_map(|batch| batch.alternatives.iter().map(|(_, ticks)| *ticks).min())
                .max()
                .unwrap_or(0);
            sum.checked_add(ticks)
                .ok_or_else(|| SimError("intrinsic compute duration lower bound overflows".into()))
        })
    }
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
