// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::{
    CapabilityKey, ComputeBatch, ComputeCandidate, ComputeModel, ComputeWork, ExecutionPlan,
    ExecutionUnitId, MatrixServiceTile, MatrixWorkShape, TypedWork, WorkStage,
};
use crate::processing_element::MachineOpCounts;
use crate::processing_element::tensors::dtype::DataType;

impl ComputeModel {
    /// Select typed capabilities and compute aggregate service times.
    pub(in crate::processing_element) fn lower(
        &self,
        work: &ComputeWork,
    ) -> Result<ExecutionPlan, SimError> {
        let plan = ExecutionPlan {
            stages: work
                .stages
                .iter()
                .map(|stage| self.lower_stage(stage))
                .collect::<Result<_, _>>()?,
        };
        // Validate total accounting across independent execution units and
        // stage barriers.
        plan.stages
            .iter()
            .flatten()
            .try_fold(MachineOpCounts::default(), |counts, batch| {
                counts.checked_add(batch.machine_ops.clone())
            })?;
        Ok(plan)
    }

    fn lower_stage(&self, stage: &WorkStage) -> Result<Vec<ComputeBatch>, SimError> {
        let mut batches: BTreeMap<ExecutionUnitId, (f64, MachineOpCounts, u64)> = BTreeMap::new();
        let mut flexible = Vec::new();
        for work in &stage.operations {
            let (candidates, accumulator_bytes) = self.select_work_candidates(work)?;
            if work.count == 0 {
                continue;
            }
            if candidates.len() > 1 {
                flexible.push(build_flexible_batch(work, &candidates, accumulator_bytes)?);
                continue;
            }
            let candidate = candidates[0];
            let (time, counts, bytes) = batches.entry(candidate.execution_unit).or_default();
            *bytes = (*bytes).max(accumulator_bytes);
            *time += service_count(work, candidate.matrix_service_tile)? as f64 / candidate.rate;
            *counts = counts
                .clone()
                .checked_add(work.machine_ops(candidate.flops_per_op)?)?;
        }
        let mut fixed = batches
            .into_iter()
            .map(|(execution_unit, (time, counts, bytes))| {
                build_fixed_batch(execution_unit, time, counts, bytes)
            })
            .collect::<Result<Vec<_>, _>>()?;
        fixed.extend(flexible);
        Ok(fixed)
    }

    fn select_work_candidates(
        &self,
        work: &TypedWork,
    ) -> Result<(Vec<ComputeCandidate>, u64), SimError> {
        validate_matrix_work(work)?;
        let mut candidates = self.select(&work.signature);
        let bytes = required_accumulator_bytes(work, &candidates)?;
        if work.count == 0 {
            return Ok((candidates, bytes));
        }
        if candidates.is_empty() {
            return Err(SimError(format!(
                "unsupported capability {:?}",
                work.signature
            )));
        }
        if candidates
            .iter()
            .any(|candidate| candidate.flops_per_op != candidates[0].flops_per_op)
        {
            return sim_error!(
                "operation {:?} has inconsistent flop costs across execution units",
                work.signature
            );
        }
        candidates.retain(|candidate| {
            candidate
                .matrix_accumulator_capacity_bytes
                .is_none_or(|capacity| bytes <= capacity)
        });
        if candidates.is_empty() {
            return sim_error!("matrix accumulator capacity cannot hold logical output tile");
        }
        Ok((candidates, bytes))
    }
}

fn validate_matrix_work(work: &TypedWork) -> Result<(), SimError> {
    if matches!(work.signature, CapabilityKey::MatrixMac { .. }) && work.matrix_shape.is_none() {
        return sim_error!("matrix_mac requires logical matrix shape");
    }
    if let Some(shape) = work.matrix_shape
        && (!matches!(work.signature, CapabilityKey::MatrixMac { .. })
            || work.signature.inputs().len() != 2
            || shape.logical_macs()? != work.count)
    {
        return sim_error!("matrix shape does not match logical work");
    }
    Ok(())
}

fn required_accumulator_bytes(
    work: &TypedWork,
    candidates: &[ComputeCandidate],
) -> Result<u64, SimError> {
    let CapabilityKey::MatrixMac { accumulator, .. } = &work.signature else {
        return Ok(0);
    };
    if !candidates
        .iter()
        .any(|candidate| candidate.matrix_accumulator_capacity_bytes.is_some())
    {
        return Ok(0);
    }
    let shape = work.matrix_shape.ok_or_else(|| {
        SimError("matrix accumulator capacity requires logical matrix shape".into())
    })?;
    accumulator_bytes(shape, *accumulator)
}

fn build_flexible_batch(
    work: &TypedWork,
    candidates: &[ComputeCandidate],
    accumulator_bytes: u64,
) -> Result<ComputeBatch, SimError> {
    let first = candidates[0];
    let first_time = service_count(work, first.matrix_service_tile)? as f64 / first.rate;
    let machine_ops = work.machine_ops(first.flops_per_op)?;
    let mut alternatives = Vec::with_capacity(candidates.len());
    alternatives.push((first.execution_unit, round_up_service_ticks(first_time)?));
    for candidate in &candidates[1..] {
        let time = service_count(work, candidate.matrix_service_tile)? as f64 / candidate.rate;
        alternatives.push((candidate.execution_unit, round_up_service_ticks(time)?));
    }
    alternatives.sort_by_key(|(unit, ticks)| (*ticks, *unit));
    Ok(ComputeBatch {
        matrix_accumulator_bytes: accumulator_bytes,
        alternatives,
        machine_ops,
    })
}

fn build_fixed_batch(
    execution_unit: ExecutionUnitId,
    time: f64,
    machine_ops: MachineOpCounts,
    matrix_accumulator_bytes: u64,
) -> Result<ComputeBatch, SimError> {
    let ticks = round_up_service_ticks(time)?;
    Ok(ComputeBatch {
        matrix_accumulator_bytes,
        alternatives: vec![(execution_unit, ticks)],
        machine_ops,
    })
}

fn round_up_service_ticks(time: f64) -> Result<u64, SimError> {
    let ticks = time.ceil();
    // u64::MAX rounds to 2^64 in f64: equality is out of range too.
    if !ticks.is_finite() || ticks >= u64::MAX as f64 || ticks < 1.0 {
        return sim_error!("compute duration cannot be represented in ticks");
    }
    Ok(ticks as u64)
}

fn service_count(work: &TypedWork, tile: Option<MatrixServiceTile>) -> Result<usize, SimError> {
    match tile {
        Some(tile) => tile.service_macs(work.matrix_shape.ok_or_else(|| {
            SimError("matrix service policy requires logical matrix shape".into())
        })?),
        None => Ok(work.count),
    }
}

fn accumulator_bytes(shape: MatrixWorkShape, accumulator: DataType) -> Result<u64, SimError> {
    let bytes = shape
        .batches
        .checked_mul(shape.m)
        .and_then(|bm| bm.checked_mul(shape.n))
        .and_then(|elements| elements.checked_mul(accumulator.num_bits()))
        .ok_or_else(|| SimError("matrix accumulator byte demand overflows".into()))?
        .div_ceil(8);
    u64::try_from(bytes).map_err(|_| SimError("matrix accumulator byte demand exceeds u64".into()))
}
