// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeSet;
use std::ops::Range;
use std::rc::Rc;

use futures::future::try_join_all;
use gwr_engine::sim_error;
use gwr_engine::time::clock::phase;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::debug;
use gwr_track::entity::EntityGroup;

use super::{
    TaskRunner, count_window_chunk_operations, find_read, slice_view, window_all_in_range,
    window_chunk_access_ranges,
};
use crate::processing_element::activity::ActivityLanes;
use crate::processing_element::compute::{ComputeWork, ExecutionPlan};
use crate::processing_element::program::syntax::{ExprSpec, ReductionOp};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ValidatedRegion;
use crate::processing_element::task::ProgramTaskConfig;

impl TaskRunner {
    pub(super) async fn run_program_assignment(
        &self,
        config: &ProgramTaskConfig,
        region: &ValidatedRegion,
        tile: &RegionTile,
        group: &Rc<EntityGroup>,
        assignment_index: usize,
    ) -> SimResult {
        emit_program_assignment(
            &self.compute_model,
            config,
            region,
            tile,
            Some(group),
            assignment_index,
            &mut super::program_pipeline::AssignmentIo::Live(self),
        )
        .await
    }

    pub(super) async fn run_program_plan(
        &self,
        config: &ProgramTaskConfig,
        tile: &RegionTile,
        assignment: &str,
        group: &Rc<EntityGroup>,
        plan: ExecutionPlan,
    ) -> SimResult {
        for stage in plan.stages {
            try_join_all(stage.into_iter().map(|batch| async move {
                self.clock.wait_phase(phase::END).await;
                let waiting = ActivityLanes::begin_in_group(&self.activity_lanes.compute_wait, &format!("{} {assignment} compute wait", config.id), group);
                let eligible = batch.alternatives.iter().map(|(engine, _)| *engine).collect::<Vec<_>>();
                let reservation = self.compute_scheduler.acquire_any_with_accumulator(
                    &eligible, batch.matrix_accumulator_bytes).await?;
                let engine = reservation.engine();
                let ticks = batch.alternatives.iter().find(|(candidate, _)| *candidate == engine)
                    .ok_or_else(|| SimError("selected engine missing from batch".into()))?.1;
                drop(waiting);
                self.clock.wait_phase(phase::END).await;
                self.clock.tick_now().tick().checked_add(ticks)
                    .ok_or_else(|| SimError(format!("{} {assignment} compute completion tick overflows", config.id)))?;
                debug!(self.entity; "{} tile {:?} assignment {assignment} begin", config.id, tile.output_intervals);
                let activity = ActivityLanes::begin_in_group(&self.activity_lanes.compute[engine.0], &format!("{} {assignment} compute", config.id), group);
                if let Some(monitor) = &self.flop_monitor {
                    monitor.record_interval(ticks, batch.machine_ops.flop_equivalents() as f64);
                }
                self.clock.wait_ticks(ticks)?.await;
                debug!(self.entity; "{} tile {:?} assignment {assignment} end", config.id, tile.output_intervals);
                drop(activity);
                drop(reservation);
                let mut stats = self.stats.borrow_mut();
        if let Some(counters)=&mut stats.compute_actions { counters.record_batch(&batch, engine)?; }
                stats.machine_ops = stats.machine_ops.clone().checked_add(batch.machine_ops)?;
                Ok::<(), SimError>(())
            })).await?;
        }
        Ok(())
    }
}

struct ReductionWork<'a> {
    over: &'a [String],
    body_op: Option<&'a str>,
    operands: Vec<&'a ExprSpec>,
    matrix_mac: bool,
}

fn collect_read_exprs<'a>(expr: &'a ExprSpec, reads: &mut Vec<&'a ExprSpec>) {
    match expr {
        ExprSpec::Read { .. } => reads.push(expr),
        ExprSpec::Call { args, .. } => {
            for arg in args {
                collect_read_exprs(arg, reads);
            }
        }
        _ => {}
    }
}

/// The same lazy semantic source feeds ordinary execution and the pipeline.
#[expect(clippy::too_many_lines)]
pub(super) async fn emit_program_assignment(
    model: &crate::processing_element::compute::ComputeModel,
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
    group: Option<&Rc<EntityGroup>>,
    assignment_index: usize,
    io: &mut super::program_pipeline::AssignmentIo<'_>,
) -> SimResult {
    let assignment = &config.program.assignments[assignment_index];
    if matches!(
        assignment.expr,
        ExprSpec::Read { .. } | ExprSpec::Call { .. }
    ) {
        let mut names = BTreeSet::new();
        super::collect_read_names(&assignment.expr, &mut names);
        for name in names {
            if config.program.metadata_inputs.contains(&name)
                || !super::tile::is_reduction_input(config, &name)
            {
                continue;
            }
            let Some(index) = config
                .program
                .inputs
                .iter()
                .position(|input| input == &name)
            else {
                continue;
            };
            let indices = find_read(&assignment.expr, &name).unwrap();
            let view = if indices.iter().any(|index| {
                matches!(
                    index,
                    crate::processing_element::program::syntax::IndexSpec::Affine { .. }
                )
            }) {
                super::window_input_view(&config.inputs[index], indices, tile, region)?
            } else {
                slice_view(&config.inputs[index], indices, &tile.output_intervals)?
            };
            io.staged_view(&view, &format!("{} {name} phase reread", config.id), group)
                .await?;
        }
    }
    if matches!(assignment.expr, ExprSpec::Read { .. }) {
        return Ok(());
    }
    if let ExprSpec::Call { .. } = &assignment.expr {
        let count = assignment.axes.iter().try_fold(1usize, |count, axis| {
            count
                .checked_mul(
                    tile.output_intervals
                        .get(axis)
                        .map_or(region.extent(axis).unwrap(), Range::len),
                )
                .ok_or_else(|| SimError("tile operation count overflows".into()))
        })?;
        let dtype = region.value(&assignment.primary_result()).unwrap().dtype;
        let work = super::helpers::expression_work(&assignment.expr, region, dtype, count)?.0;
        let plan = model.lower(&work)?;
        io.run_program_plan(config, tile, &assignment.primary_result(), group, plan)
            .await?;
        return Ok(());
    }
    if let ExprSpec::Reduce {
        op: ReductionOp::Min | ReductionOp::Max,
        value,
        over,
    } = &assignment.expr
    {
        if matches!(&**value, ExprSpec::Call { .. }) {
            let outputs = assignment.axes.iter().try_fold(1usize, |count, axis| {
                count
                    .checked_mul(
                        tile.output_intervals
                            .get(axis)
                            .map_or(region.extent(axis).unwrap(), Range::len),
                    )
                    .ok_or_else(|| SimError("extremum output count overflows".into()))
            })?;
            let result_dtype = assignment.output.as_ref().map_or_else(
                || super::helpers::expression_read_dtype(value, region),
                |name| region.value(name).unwrap().dtype,
            );
            let mut names = BTreeSet::new();
            super::collect_read_names(value, &mut names);
            let mut leaves = Vec::new();
            collect_read_exprs(value, &mut leaves);
            let excluded = leaves.iter().find(|expr| {
                matches!(
                    expr,
                    ExprSpec::Read {
                        bounds: crate::processing_element::program::syntax::BoundsMode::Exclude,
                        ..
                    }
                )
            });
            let primary_binding = excluded.and_then(|expr| {
                let ExprSpec::Read {
                    tensor, indices, ..
                } = expr
                else {
                    unreachable!()
                };
                config
                    .program
                    .inputs
                    .iter()
                    .position(|input| input == tensor)
                    .map(|index| (indices, &config.inputs[index]))
            });
            let in_range = if let Some((indices, input)) = primary_binding {
                window_all_in_range(indices, tile, region, input)?
            } else {
                true
            };
            let mut seen = BTreeSet::<Vec<usize>>::new();
            for (chunk_index, chunk) in tile.reduction_chunks_for(over).enumerate() {
                for name in &names {
                    if config.program.metadata_inputs.contains(name) {
                        continue;
                    }
                    let Some(index) = config.program.inputs.iter().position(|input| input == name)
                    else {
                        continue;
                    };
                    let indices = find_read(value, name).unwrap();
                    let (ranges, bytes) = window_chunk_access_ranges(
                        &config.inputs[index],
                        indices,
                        tile,
                        region,
                        &chunk,
                    )?;
                    if !ranges.is_empty() {
                        io.staged_access_ranges(
                            AccessType::ReadRequest,
                            ranges,
                            bytes,
                            &format!("{} {name} extremum chunk {chunk_index} read", config.id),
                            group,
                        )
                        .await?;
                    }
                }
                let reduction_len = chunk.values().try_fold(1usize, |count, range| {
                    count
                        .checked_mul(range.len())
                        .ok_or_else(|| SimError("extremum chunk count overflows".into()))
                })?;
                let (elements, comparisons) = if let Some((indices, input)) = primary_binding {
                    count_window_chunk_operations(
                        &[(indices, input)],
                        &assignment.axes,
                        over,
                        tile,
                        region,
                        &chunk,
                        chunk_index == 0,
                        in_range,
                        &mut seen,
                    )?
                } else {
                    let elements = outputs
                        .checked_mul(reduction_len)
                        .ok_or_else(|| SimError("extremum body count overflows".into()))?;
                    let comparisons = outputs
                        .checked_mul(reduction_len - usize::from(chunk_index == 0))
                        .ok_or_else(|| SimError("extremum comparison count overflows".into()))?;
                    (elements, comparisons)
                };
                let work =
                    super::helpers::expression_work(value, region, result_dtype, elements)?.0;
                let mut plan = model.lower(&work)?;
                if comparisons != 0 {
                    plan.stages.extend(
                        model
                            .lower(&ComputeWork::operation(
                                "compare",
                                vec![result_dtype, result_dtype],
                                comparisons,
                            ))?
                            .stages,
                    );
                }
                io.run_program_plan(config, tile, &assignment.primary_result(), group, plan)
                    .await?;
            }
            if !in_range && seen.len() != outputs {
                return sim_error!("empty nested min/max window");
            }
            return Ok(());
        }
        let ExprSpec::Read {
            tensor, indices, ..
        } = &**value
        else {
            return sim_error!(
                "{} min/max reduction requires a read",
                assignment.primary_result()
            );
        };
        if !config.program.inputs.contains(tensor)
            || config.program.metadata_inputs.contains(tensor)
        {
            let outputs = assignment.axes.iter().try_fold(1usize, |count, axis| {
                count
                    .checked_mul(
                        tile.output_intervals
                            .get(axis)
                            .map_or(region.extent(axis).unwrap(), Range::len),
                    )
                    .ok_or_else(|| SimError("local reduction output count overflows".into()))
            })?;
            for (chunk_index, chunk) in tile.reduction_chunks_for(over).enumerate() {
                let reduction_len = chunk.values().try_fold(1usize, |count, range| {
                    count
                        .checked_mul(range.len())
                        .ok_or_else(|| SimError("local reduction size overflows".into()))
                })?;
                let comparisons = outputs
                    .checked_mul(reduction_len - usize::from(chunk_index == 0))
                    .ok_or_else(|| SimError("local reduction comparisons overflow".into()))?;
                if comparisons != 0 {
                    let dtype = region.value(tensor).unwrap().dtype;
                    let plan = model.lower(&ComputeWork::operation(
                        "compare",
                        vec![dtype, dtype],
                        comparisons,
                    ))?;
                    io.run_program_plan(config, tile, &assignment.primary_result(), group, plan)
                        .await?;
                }
            }
            return Ok(());
        }
        let input_index = config
            .program
            .inputs
            .iter()
            .position(|name| name == tensor)
            .ok_or_else(|| SimError(format!("max source {tensor} is not a boundary input")))?;
        let input = &config.inputs[input_index];
        let in_range = window_all_in_range(indices, tile, region, input)?;
        let mut seen = BTreeSet::<Vec<usize>>::new();
        for (chunk_index, chunk) in tile.reduction_chunks_for(over).enumerate() {
            let (ranges, bytes) = window_chunk_access_ranges(input, indices, tile, region, &chunk)?;
            if !ranges.is_empty() {
                io.staged_access_ranges(
                    AccessType::ReadRequest,
                    ranges,
                    bytes,
                    &format!("{} {tensor} window chunk {chunk_index} read", config.id),
                    group,
                )
                .await?;
            }
            let (_, comparisons) = count_window_chunk_operations(
                &[(indices, input)],
                &assignment.axes,
                over,
                tile,
                region,
                &chunk,
                chunk_index == 0,
                in_range,
                &mut seen,
            )?;
            let plan = if comparisons == 0 {
                // Initialization still consumes this input generation.
                // Keep its chunk boundary even when no ALU work is needed.
                ExecutionPlan { stages: vec![] }
            } else {
                let dtype = region.value(tensor).unwrap().dtype;
                model.lower(&ComputeWork::operation(
                    "compare",
                    vec![dtype, dtype],
                    comparisons,
                ))?
            };
            io.run_program_plan(config, tile, &assignment.primary_result(), group, plan)
                .await?;
        }
        if !in_range {
            let outputs = assignment.axes.iter().try_fold(1usize, |count, axis| {
                count
                    .checked_mul(
                        tile.output_intervals
                            .get(axis)
                            .map_or(region.extent(axis).unwrap(), Range::len),
                    )
                    .ok_or_else(|| SimError("max output count overflows".into()))
            })?;
            if seen.len() != outputs {
                return sim_error!("empty min/max window");
            }
        }
        return Ok(());
    }
    let reduction = match &assignment.expr {
        ExprSpec::Reduce {
            op: ReductionOp::Sum,
            over,
            value,
        } => {
            let (body_op, operands) = match &**value {
                ExprSpec::Read { .. } => (None, vec![&**value]),
                ExprSpec::Call { op, .. } => {
                    let mut reads = Vec::new();
                    collect_read_exprs(value, &mut reads);
                    (Some(op.as_str()), reads)
                }
                _ => {
                    return sim_error!(
                        "{} unsupported reduction body",
                        assignment.primary_result()
                    );
                }
            };
            Some(ReductionWork {
                over,
                body_op,
                operands,
                matrix_mac: false,
            })
        }
        ExprSpec::MatrixMac { args, over, .. } => Some(ReductionWork {
            over,
            body_op: None,
            operands: args.iter().collect(),
            matrix_mac: true,
        }),
        _ => None,
    };
    if let Some(ReductionWork {
        over,
        body_op,
        operands,
        matrix_mac,
    }) = reduction
    {
        let names = operands
            .iter()
            .map(|operand| {
                let ExprSpec::Read { tensor, .. } = operand else {
                    unreachable!()
                };
                tensor.as_str()
            })
            .collect::<Vec<_>>();
        let lhs = names[0];
        let rhs = *names.get(1).unwrap_or(&lhs);
        let outputs = assignment.axes.iter().try_fold(1usize, |count, axis| {
            count
                .checked_mul(
                    tile.output_intervals
                        .get(axis)
                        .map_or(region.extent(axis).unwrap(), Range::len),
                )
                .ok_or_else(|| SimError("tile output count overflows".into()))
        })?;
        let excluded = operands
            .iter()
            .filter_map(|operand| {
                let ExprSpec::Read {
                    tensor,
                    indices,
                    bounds,
                } = operand
                else {
                    unreachable!()
                };
                if *bounds != crate::processing_element::program::syntax::BoundsMode::Exclude {
                    return None;
                }
                config
                    .program
                    .inputs
                    .iter()
                    .position(|input| input == tensor)
                    .map(|index| (indices.as_slice(), &config.inputs[index]))
            })
            .collect::<Vec<_>>();
        let in_range = excluded
            .iter()
            .try_fold(true, |present, (indices, input)| {
                Ok::<bool, SimError>(present && window_all_in_range(indices, tile, region, input)?)
            })?;
        let mut seen = BTreeSet::new();
        for (chunk_index, chunk) in tile.reduction_chunks_for(over).enumerate() {
            let mut intervals = tile.output_intervals.clone();
            intervals.extend(chunk.clone());
            let (elements, additions) = count_window_chunk_operations(
                &excluded,
                &assignment.axes,
                over,
                tile,
                region,
                &chunk,
                chunk_index == 0,
                in_range,
                &mut seen,
            )?;
            let matrix_plan = super::matrix_shape::work(model, assignment, region, &intervals)?
                .map(|work| {
                    if work.stages[0].operations[0].count != elements {
                        return Err(SimError(
                            "matrix shape does not match chunk operations".into(),
                        ));
                    }
                    model.lower(&work)
                })
                .transpose()?;
            for name in names.iter().copied().collect::<BTreeSet<_>>() {
                if config
                    .program
                    .metadata_inputs
                    .iter()
                    .any(|input| input == name)
                    || !config.program.inputs.iter().any(|input| input == name)
                {
                    continue;
                }
                let index = config
                    .program
                    .inputs
                    .iter()
                    .position(|input| input == name)
                    .ok_or_else(|| {
                        SimError(format!("reduction operand {name} is not a boundary input"))
                    })?;
                let indices = find_read(&assignment.expr, name).unwrap();
                if indices.iter().any(|index| {
                    matches!(
                        index,
                        crate::processing_element::program::syntax::IndexSpec::Affine { .. }
                    )
                }) {
                    let (ranges, bytes) = window_chunk_access_ranges(
                        &config.inputs[index],
                        indices,
                        tile,
                        region,
                        &chunk,
                    )?;
                    if !ranges.is_empty() {
                        io.staged_access_ranges(
                            AccessType::ReadRequest,
                            ranges,
                            bytes,
                            &format!("{} {name} chunk {chunk_index} read", config.id),
                            group,
                        )
                        .await?;
                    }
                } else {
                    let view = slice_view(&config.inputs[index], indices, &intervals)?;
                    io.staged_view(
                        &view,
                        &format!("{} {name} chunk {chunk_index} read", config.id),
                        group,
                    )
                    .await?;
                }
            }
            let dtype = region.value(&assignment.primary_result()).unwrap().dtype;
            let lhs_dtype = region.value(lhs).unwrap().dtype;
            let rhs_dtype = region.value(rhs).unwrap().dtype;
            let mut plan = if let Some(plan) = matrix_plan {
                plan
            } else if matrix_mac {
                model.lower(
                    &ComputeWork::operation("matrix_mac", vec![lhs_dtype, rhs_dtype], elements)
                        .with_execution(match &assignment.expr {
                            ExprSpec::MatrixMac { execution, .. } => execution.clone(),
                            _ => None,
                        }),
                )?
            } else if body_op.is_some() {
                let ExprSpec::Reduce { value, .. } = &assignment.expr else {
                    unreachable!()
                };
                let work = super::helpers::expression_work(value, region, dtype, elements)?.0;
                model.lower(&work)?
            } else {
                ExecutionPlan { stages: vec![] }
            };
            if !matrix_mac && additions != 0 {
                let extra = model.lower(&ComputeWork::operation(
                    "add",
                    vec![dtype, dtype],
                    additions,
                ))?;
                plan.stages.extend(extra.stages);
            }
            io.run_program_plan(config, tile, &assignment.primary_result(), group, plan)
                .await?;
        }
        if let ExprSpec::MatrixMac {
            execution: Some(mode),
            ..
        } = &assignment.expr
        {
            let output = region.value(&assignment.primary_result()).unwrap().dtype;
            if mode.accumulator != output {
                let plan = model.lower(&ComputeWork::operation(
                    "convert",
                    vec![mode.accumulator],
                    outputs,
                ))?;
                io.run_program_plan(config, tile, &assignment.primary_result(), group, plan)
                    .await?;
            }
        }
        return Ok(());
    }
    sim_error!(
        "{} unsupported expression in region runner",
        assignment.primary_result()
    )
}
