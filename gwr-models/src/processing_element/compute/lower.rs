// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::{ComputeModel, ComputeWork, EngineBatch, EngineId, ExecutionPlan, WorkStage};
use crate::processing_element::MachineOpCounts;

impl ComputeModel {
    /// Select typed capabilities and compute aggregate service times.
    pub fn lower(&self, work: &ComputeWork) -> Result<ExecutionPlan, SimError> {
        let plan = ExecutionPlan {
            stages: work
                .stages
                .iter()
                .map(|stage| self.lower_stage(stage))
                .collect::<Result<_, _>>()?,
        };
        // Validate total accounting across independent engines and stage
        // barriers.
        plan.stages
            .iter()
            .flatten()
            .try_fold(MachineOpCounts::default(), |counts, batch| {
                counts.checked_add(batch.machine_ops.clone())
            })?;
        Ok(plan)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Lower each supported instruction family through one exhaustive dispatch"
    )]
    fn lower_stage(&self, stage: &WorkStage) -> Result<Vec<EngineBatch>, SimError> {
        let mut batches: BTreeMap<
            EngineId,
            (f64, MachineOpCounts, u64, Vec<super::ComputeAction>),
        > = BTreeMap::new();
        let mut flexible = Vec::new();
        for work in &stage.operations {
            if let Some(shape) = work.matrix_shape
                && (work.signature.op != "matrix_mac"
                    || work.signature.inputs.len() != 2
                    || shape.logical_macs()? != work.count)
            {
                return sim_error!("matrix shape does not match logical work");
            }
            let mut candidates = self.select(&work.signature);
            if candidates
                .iter()
                .any(|candidate| candidate.matrix_service_tile.is_some())
                && work.matrix_shape.is_none()
            {
                return sim_error!("matrix service policy requires logical matrix shape");
            }
            let capacity_enabled = candidates
                .iter()
                .any(|candidate| candidate.matrix_accumulator_capacity_bytes.is_some())
                && work.signature.op == "matrix_mac";
            let matrix_accumulator_bytes = if capacity_enabled {
                accumulator_bytes(work)?
            } else {
                0
            };
            if work.count == 0 {
                continue;
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
                    "operation {:?} has inconsistent flop costs across engines",
                    work.signature
                );
            }
            if capacity_enabled {
                candidates.retain(|candidate| {
                    candidate
                        .matrix_accumulator_capacity_bytes
                        .is_none_or(|capacity| matrix_accumulator_bytes <= capacity)
                });
                if candidates.is_empty() {
                    return sim_error!(
                        "matrix accumulator capacity cannot hold logical output tile"
                    );
                }
            }
            if candidates.len() > 1 {
                let flops_per_op = candidates[0].flops_per_op;
                let mut options = candidates
                    .into_iter()
                    .map(|candidate| {
                        timed_batch(
                            candidate.engine,
                            service_count(work, candidate.matrix_service_tile)? as f64
                                / candidate.rate,
                            work.machine_ops(flops_per_op)?,
                            matrix_accumulator_bytes,
                            vec![action(work, candidate.matrix_service_tile)?],
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                options.sort_by_key(|batch| (batch.ticks, batch.engine));
                let mut batch = options.remove(0);
                for option in options {
                    batch.alternatives.push((option.engine, option.ticks));
                    batch.actions.extend(option.actions);
                }
                flexible.push(batch);
                continue;
            }
            let candidate = candidates[0];
            let (time, counts, bytes, actions) = batches.entry(candidate.engine).or_default();
            actions.push(action(work, candidate.matrix_service_tile)?);
            *bytes = (*bytes).max(matrix_accumulator_bytes);
            *time += service_count(work, candidate.matrix_service_tile)? as f64 / candidate.rate;
            *counts = counts
                .clone()
                .checked_add(work.machine_ops(candidate.flops_per_op)?)?;
        }
        let mut fixed = batches
            .into_iter()
            .map(|(engine, (time, counts, bytes, actions))| {
                timed_batch(engine, time, counts, bytes, actions)
            })
            .collect::<Result<Vec<_>, _>>()?;
        fixed.extend(flexible);
        Ok(fixed)
    }
}

fn timed_batch(
    engine: EngineId,
    time: f64,
    machine_ops: MachineOpCounts,
    matrix_accumulator_bytes: u64,
    actions: Vec<super::ComputeAction>,
) -> Result<EngineBatch, SimError> {
    let ticks = time.ceil();
    // u64::MAX rounds to 2^64 in f64: equality is out of range too.
    if !ticks.is_finite() || ticks >= u64::MAX as f64 || ticks < 1.0 {
        return sim_error!("compute duration cannot be represented in ticks");
    }
    Ok(EngineBatch {
        actions: BTreeMap::from([(engine, actions)]),
        matrix_accumulator_bytes,
        engine,
        alternatives: vec![(engine, ticks as u64)],
        ticks: ticks as u64,
        machine_ops,
    })
}

fn service_count(
    work: &super::TypedWork,
    tile: Option<super::MatrixServiceTile>,
) -> Result<usize, SimError> {
    match tile {
        Some(tile) => tile.service_macs(work.matrix_shape.ok_or_else(|| {
            SimError("matrix service policy requires logical matrix shape".into())
        })?),
        None => Ok(work.count),
    }
}

fn accumulator_bytes(work: &super::TypedWork) -> Result<u64, SimError> {
    let shape = work.matrix_shape.ok_or_else(|| {
        SimError("matrix accumulator capacity requires logical matrix shape".into())
    })?;
    let mode = work.signature.execution.as_ref().ok_or_else(|| {
        SimError("matrix accumulator capacity requires explicit execution".into())
    })?;
    let bytes = shape
        .m
        .checked_mul(shape.n)
        .and_then(|elements| elements.checked_mul(mode.accumulator.num_bits()))
        .ok_or_else(|| SimError("matrix accumulator byte demand overflows".into()))?
        .div_ceil(8);
    u64::try_from(bytes).map_err(|_| SimError("matrix accumulator byte demand exceeds u64".into()))
}

fn action(
    work: &super::TypedWork,
    tile: Option<super::MatrixServiceTile>,
) -> Result<super::ComputeAction, SimError> {
    Ok(super::ComputeAction {
        signature: work.signature.clone(),
        logical_operations: u64::try_from(work.count)
            .map_err(|_| SimError("logical work exceeds u64".into()))?,
        service_operations: u64::try_from(service_count(work, tile)?)
            .map_err(|_| SimError("service work exceeds u64".into()))?,
    })
}
