// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::EngineId;
use crate::processing_element::MachineOpCounts;
use crate::processing_element::tensors::dtype::DataType;

/// Sequential stages of named operations before selecting engines.
#[derive(Clone, Debug)]
pub struct ComputeWork {
    pub stages: Vec<WorkStage>,
}

impl ComputeWork {
    /// Matrix work with checked logical geometry; service shape is selected per
    /// candidate.
    pub fn matrix_mac(
        inputs: Vec<DataType>,
        shape: MatrixWorkShape,
    ) -> Result<Self, gwr_engine::types::SimError> {
        if inputs.len() != 2 {
            return Err(gwr_engine::types::SimError(
                "matrix shape requires two operands".into(),
            ));
        }
        let count = shape.logical_macs()?;
        let mut work = Self::operation("matrix_mac", inputs, count);
        work.stages[0].operations[0].matrix_shape = Some(shape);
        Ok(work)
    }

    #[must_use]
    pub fn operation(op: impl Into<String>, inputs: Vec<DataType>, count: usize) -> Self {
        Self {
            stages: vec![WorkStage {
                operations: vec![TypedWork::operation(op, inputs, count)],
            }],
        }
    }

    /// Select explicit instruction semantics for this work's operations.
    #[must_use]
    pub fn with_execution(mut self, execution: Option<super::ExecutionMode>) -> Self {
        for (operation, execution) in self
            .stages
            .iter_mut()
            .flat_map(|stage| &mut stage.operations)
            .zip(std::iter::repeat(execution))
        {
            operation.signature.execution = execution;
        }
        self
    }

    #[must_use]
    pub fn parallel(works: Vec<Self>) -> Self {
        let operations = works
            .into_iter()
            .flat_map(|work| work.stages)
            .flat_map(|stage| stage.operations)
            .collect();
        Self {
            stages: vec![WorkStage { operations }],
        }
    }
}

/// Independent work; a barrier separates consecutive stages.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkStage {
    /// Independent typed demands, grouped into engine batches during lowering.
    pub operations: Vec<TypedWork>,
}

/// A number of operations of one exact signature.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedWork {
    /// Exact capability required.
    pub signature: CapabilityKey,
    /// Logical operation count.
    pub count: usize,
    /// Actual logical output tile and contraction chunk dimensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_shape: Option<MatrixWorkShape>,
}

impl TypedWork {
    #[must_use]
    pub fn operation(op: impl Into<String>, inputs: Vec<DataType>, count: usize) -> Self {
        Self {
            signature: CapabilityKey {
                op: op.into(),
                inputs,
                execution: None,
            },
            count,
            matrix_shape: None,
        }
    }

    pub(crate) fn machine_ops(
        &self,
        flops_per_op: usize,
    ) -> Result<MachineOpCounts, gwr_engine::types::SimError> {
        MachineOpCounts::single(&self.signature.op, self.count, flops_per_op)
    }
}

/// Logical geometry for a canonical unbatched matrix contraction.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixWorkShape {
    pub m: usize,
    pub n: usize,
    pub k: usize,
}

impl MatrixWorkShape {
    /// Checked logical MACs; zero dimensions are malformed, not empty work.
    pub fn logical_macs(self) -> Result<usize, gwr_engine::types::SimError> {
        if self.m == 0 || self.n == 0 || self.k == 0 {
            return Err(gwr_engine::types::SimError(
                "matrix shape dimensions must be positive".into(),
            ));
        }
        self.m
            .checked_mul(self.n)
            .and_then(|mn| mn.checked_mul(self.k))
            .ok_or_else(|| gwr_engine::types::SimError("matrix shape MAC count overflows".into()))
    }
}

/// An exact typed operation, used for runtime matching (never contains sets).
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityKey {
    /// Operation identity.
    pub op: String,
    /// Ordered arithmetic operands.
    pub inputs: Vec<DataType>,
    /// Explicit instruction semantics; omitted for legacy capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<super::ExecutionMode>,
}

/// Sequential barriers containing independent engine batches.
#[derive(Clone, Debug)]
pub struct ExecutionPlan {
    /// Batches in stable engine order, grouped into sequential stages.
    pub stages: Vec<Vec<EngineBatch>>,
}

impl ExecutionPlan {
    /// Uncontended service time; runtime scheduling adds contention.
    pub fn service_ticks(&self) -> Result<u64, gwr_engine::types::SimError> {
        self.stages.iter().try_fold(0_u64, |sum, stage| {
            let mut occupied = BTreeMap::<EngineId, u64>::new();
            for batch in stage {
                let (engine, finish) = batch
                    .alternatives
                    .iter()
                    .map(|(engine, ticks)| {
                        occupied
                            .get(engine)
                            .copied()
                            .unwrap_or(0)
                            .checked_add(*ticks)
                            .map(|finish| (*engine, finish))
                            .ok_or_else(|| {
                                gwr_engine::types::SimError("compute duration overflows".into())
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .min_by_key(|(engine, finish)| (*finish, *engine))
                    .ok_or_else(|| {
                        gwr_engine::types::SimError("compute batch has no engines".into())
                    })?;
                occupied.insert(engine, finish);
            }
            sum.checked_add(occupied.values().copied().max().unwrap_or(0))
                .ok_or_else(|| gwr_engine::types::SimError("compute duration overflows".into()))
        })
    }
}

/// Nonpreemptive work occupying one engine.
#[derive(Clone, Debug)]
pub struct EngineBatch {
    /// Typed logical and charged service operations for each eligible engine.
    pub actions: BTreeMap<EngineId, Vec<super::ComputeAction>>,
    /// Checked logical accumulator demand, independent of charged service
    /// work and program SRAM. Zero when no matching capacity is enabled.
    pub matrix_accumulator_bytes: u64,
    /// Selected engine.
    pub engine: EngineId,
    /// Eligible engines and their service times, fastest first.
    pub alternatives: Vec<(EngineId, u64)>,
    /// Rounded service duration, excluding queue time.
    pub ticks: u64,
    /// Mathematical contributions for monitoring and statistics.
    pub machine_ops: MachineOpCounts,
}
