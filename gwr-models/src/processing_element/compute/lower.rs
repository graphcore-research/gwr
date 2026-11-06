// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::{
    ComputeBatch, ComputeCandidate, ComputeModel, ComputeWork, ExecutionPlan, ExecutionUnitId,
    MatrixServiceTile, MatrixWorkShape, TypedWork, WorkStage,
};
use crate::processing_element::MachineOpCounts;
use crate::processing_element::tensors::dtype::DataType;

#[derive(Debug, Default)]
pub(in crate::processing_element) struct PreparedCompute {
    candidates: BTreeMap<WorkSignature, Vec<ComputeCandidate>>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum WorkSignature {
    Scalar(String, Vec<DataType>),
    Matrix([DataType; 2], DataType),
}
impl From<&TypedWork> for WorkSignature {
    fn from(work: &TypedWork) -> Self {
        match work {
            TypedWork::Operation { op, inputs, .. } => Self::Scalar(op.clone(), inputs.clone()),
            TypedWork::MatrixMac {
                inputs,
                accumulator,
                ..
            } => Self::Matrix(*inputs, *accumulator),
        }
    }
}
impl PreparedCompute {
    pub(in crate::processing_element) fn prepare(
        &mut self,
        model: &ComputeModel,
        work: &ComputeWork,
    ) -> Result<MachineOpCounts, SimError> {
        lower_work(work, |operation| {
            use std::collections::btree_map::Entry;
            let candidates = match self.candidates.entry(operation.into()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(model.resolve_candidates(operation)?),
            };
            Ok(candidates.clone())
        })
        .map(|(_, counts)| counts)
    }
    pub(in crate::processing_element) fn lower(
        &self,
        work: &ComputeWork,
    ) -> Result<ExecutionPlan, SimError> {
        lower_work(work, |operation| {
            self.candidates
                .get(&operation.into())
                .cloned()
                .ok_or_else(|| SimError("unprepared operation signature".into()))
        })
        .map(|(plan, _)| plan)
    }
}
impl ComputeModel {
    pub(in crate::processing_element) fn lower(
        &self,
        work: &ComputeWork,
    ) -> Result<ExecutionPlan, SimError> {
        lower_work(work, |operation| self.resolve_candidates(operation)).map(|(plan, _)| plan)
    }
}
impl ComputeModel {
    fn resolve_candidates(&self, work: &TypedWork) -> Result<Vec<ComputeCandidate>, SimError> {
        let candidates = self.select(work);
        if candidates.is_empty() {
            return Err(SimError(format!("unsupported capability {work:?}")));
        }
        if candidates
            .iter()
            .any(|candidate| candidate.flops_per_op != candidates[0].flops_per_op)
        {
            return sim_error!(
                "operation {:?} has inconsistent flop costs across execution units",
                work
            );
        }
        Ok(candidates)
    }
}

fn lower_work(
    work: &ComputeWork,
    mut candidates: impl FnMut(&TypedWork) -> Result<Vec<ComputeCandidate>, SimError>,
) -> Result<(ExecutionPlan, MachineOpCounts), SimError> {
    let plan = ExecutionPlan {
        stages: work
            .stages
            .iter()
            .map(|stage| lower_stage(stage, &mut candidates))
            .collect::<Result<_, _>>()?,
    };
    // Validate total accounting across independent execution units and
    // stage barriers.
    let counts = plan
        .stages
        .iter()
        .flatten()
        .try_fold(MachineOpCounts::default(), |counts, batch| {
            counts.checked_add(batch.machine_ops.clone())
        })?;
    Ok((plan, counts))
}

fn lower_stage(
    stage: &WorkStage,
    candidates: &mut impl FnMut(&TypedWork) -> Result<Vec<ComputeCandidate>, SimError>,
) -> Result<Vec<ComputeBatch>, SimError> {
    let mut batches: BTreeMap<ExecutionUnitId, (f64, MachineOpCounts, u64)> = BTreeMap::new();
    let mut flexible = Vec::new();
    for work in &stage.operations {
        if work.count() == 0 {
            continue;
        }
        let candidates = candidates(work)?;
        let machine_ops = work.machine_ops(candidates[0].flops_per_op)?;
        let (candidates, accumulator_bytes) = select_work_candidates(work, candidates)?;
        if candidates.len() > 1 {
            flexible.push(build_flexible_batch(
                candidates,
                machine_ops,
                accumulator_bytes,
            ));
            continue;
        }
        let candidate = &candidates[0];
        let (time, counts, bytes) = batches.entry(candidate.execution_unit).or_default();
        *bytes = (*bytes).max(accumulator_bytes);
        *time += candidate.time;
        *counts = counts.clone().checked_add(machine_ops)?;
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

/// Checked service for one candidate, retaining fractional time for batching.
struct CandidateService {
    execution_unit: ExecutionUnitId,
    time: f64,
    ticks: u64,
}

fn select_work_candidates(
    work: &TypedWork,
    mut candidates: Vec<ComputeCandidate>,
) -> Result<(Vec<CandidateService>, u64), SimError> {
    let bytes = required_accumulator_bytes(work, &candidates)?;
    candidates.retain(|candidate| {
        candidate
            .matrix_accumulator_capacity_bytes
            .is_none_or(|capacity| bytes <= capacity)
    });
    if candidates.is_empty() {
        return sim_error!("matrix accumulator capacity cannot hold logical output tile");
    }
    let mut services = Vec::new();
    let mut rejection = None;
    for candidate in candidates {
        // Padding and tick limits belong to this unit. Filter before deciding
        // whether the surviving candidates need fixed or flexible batching.
        let service = service_count(work, candidate.matrix_service_tile).and_then(|count| {
            let time = count as f64 / candidate.rate;
            Ok(CandidateService {
                execution_unit: candidate.execution_unit,
                time,
                ticks: round_up_service_ticks(time)?,
            })
        });
        match service {
            Ok(service) => services.push(service),
            Err(error) => rejection = Some(error),
        }
    }
    if services.is_empty() {
        return Err(
            rejection.unwrap_or_else(|| SimError("no serviceable compute candidate".into()))
        );
    }
    Ok((services, bytes))
}
fn required_accumulator_bytes(
    work: &TypedWork,
    candidates: &[ComputeCandidate],
) -> Result<u64, SimError> {
    let TypedWork::MatrixMac {
        accumulator, shape, ..
    } = work
    else {
        return Ok(0);
    };
    if !candidates
        .iter()
        .any(|candidate| candidate.matrix_accumulator_capacity_bytes.is_some())
    {
        return Ok(0);
    }
    accumulator_bytes(shape.geometry(), *accumulator)
}

fn build_flexible_batch(
    candidates: Vec<CandidateService>,
    machine_ops: MachineOpCounts,
    matrix_accumulator_bytes: u64,
) -> ComputeBatch {
    let mut alternatives = candidates
        .into_iter()
        .map(|candidate| (candidate.execution_unit, candidate.ticks))
        .collect::<Vec<_>>();
    alternatives.sort_by_key(|(unit, ticks)| (*ticks, *unit));
    ComputeBatch {
        matrix_accumulator_bytes,
        alternatives,
        machine_ops,
    }
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
    match (work, tile) {
        (TypedWork::MatrixMac { shape, .. }, Some(tile)) => tile.service_macs(shape.geometry()),
        _ => Ok(work.count()),
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
