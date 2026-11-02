// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::rc::Rc;

use futures::future::try_join_all;
use gwr_engine::sim_error;
use gwr_engine::time::clock::phase::END;
use gwr_engine::types::{SimError, SimResult};
use gwr_track::debug;
use gwr_track::entity::EntityGroup;

use super::access::{InputRead, collect_assignment_input_reads};
use super::helpers::{build_scalar_expression_work, find_first_read_dtype};
use super::matrix_shape::build_matrix_work;
use super::{TaskRunner, count_window_chunk_operations, window_indices_are_in_range};
use crate::processing_element::activity::ActivityLanes;
use crate::processing_element::compute::{
    ComputeBatch, ComputeReservation, ComputeWork, ExecutionPlan,
};
use crate::processing_element::program::syntax::{
    AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ReductionOp,
};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ValidatedRegion;
use crate::processing_element::task::ProgramTaskConfig;
use crate::processing_element::tensors::TensorView;
use crate::processing_element::tensors::dtype::DataType;

impl TaskRunner {
    pub(super) async fn run_program_assignment(
        &self,
        config: &ProgramTaskConfig,
        region: &ValidatedRegion,
        tile: &RegionTile,
        group: &Rc<EntityGroup>,
        assignment_index: usize,
    ) -> SimResult {
        AssignmentExecution {
            runner: self,
            config,
            region,
            tile,
            group,
            assignment: &config.program.assignments[assignment_index],
            input_reads: collect_assignment_input_reads(config, assignment_index),
        }
        .run()
        .await
    }
}

struct AssignmentExecution<'a> {
    runner: &'a TaskRunner,
    config: &'a ProgramTaskConfig,
    region: &'a ValidatedRegion,
    tile: &'a RegionTile,
    group: &'a Rc<EntityGroup>,
    assignment: &'a AssignmentSpec,
    input_reads: Vec<InputRead<'a>>,
}

impl<'a> AssignmentExecution<'a> {
    async fn run(&self) -> SimResult {
        match &self.assignment.expr {
            ExprSpec::Read { .. } | ExprSpec::Call { .. } => self.run_scalar_expression().await,
            ExprSpec::Reduce {
                op: ReductionOp::Min | ReductionOp::Max,
                value,
                over,
            } => self.run_extremum(value, over).await,
            ExprSpec::Reduce {
                op: ReductionOp::Sum,
                ..
            }
            | ExprSpec::MatrixMac { .. } => self.run_reduction().await,
        }
    }

    async fn run_scalar_expression(&self) -> SimResult {
        self.load_inputs(&BTreeMap::new(), "phase reread").await?;
        if matches!(self.assignment.expr, ExprSpec::Read { .. }) {
            return Ok(());
        }
        let count = self.count_output_elements("tile operation count overflows")?;
        let dtype = self
            .region
            .value(&self.assignment.primary_result())
            .unwrap()
            .dtype;
        let work =
            build_scalar_expression_work(&self.assignment.expr, self.region, dtype, count)?.0;
        let plan = self.runner.compute_model.lower(&work)?;
        self.run_plan(plan).await
    }

    async fn run_extremum(&self, value: &'a ExprSpec, over: &[String]) -> SimResult {
        match value {
            ExprSpec::Call { .. } => self.run_nested_extremum(value, over).await,
            ExprSpec::Read { tensor, .. } if !self.config.program.inputs.contains(tensor) => {
                self.run_local_extremum(tensor, over).await
            }
            ExprSpec::Read {
                tensor, indices, ..
            } => self.run_window_extremum(tensor, indices, over).await,
            _ => sim_error!(
                "{} min/max reduction requires a read",
                self.assignment.primary_result()
            ),
        }
    }

    async fn run_nested_extremum(&self, value: &'a ExprSpec, over: &[String]) -> SimResult {
        let outputs = self.count_output_elements("extremum output count overflows")?;
        let result_dtype = self.assignment.output.as_ref().map_or_else(
            || find_first_read_dtype(value, self.region),
            |name| self.region.value(name).unwrap().dtype,
        );
        let primary_binding = self.find_primary_excluded_input(value);
        let in_range = if let Some((indices, input)) = primary_binding {
            window_indices_are_in_range(indices, self.tile, self.region, input)?
        } else {
            true
        };
        let mut seen = BTreeSet::<Vec<usize>>::new();
        for (chunk_index, chunk) in self.tile.reduction_chunks_for(over).enumerate() {
            self.load_inputs(&chunk, &format!("extremum chunk {chunk_index} read"))
                .await?;
            let reduction_len = chunk.values().try_fold(1usize, |count, range| {
                count
                    .checked_mul(range.len())
                    .ok_or_else(|| SimError("extremum chunk count overflows".into()))
            })?;
            let (elements, comparisons) = if let Some((indices, input)) = primary_binding {
                count_window_chunk_operations(
                    &[(indices, input)],
                    &self.assignment.axes,
                    over,
                    self.tile,
                    self.region,
                    &chunk,
                    chunk_index == 0,
                    in_range,
                    &mut seen,
                )?
            } else {
                count_extremum_operations(outputs, reduction_len, chunk_index == 0)?
            };
            let plan =
                self.build_extremum_chunk_plan(value, result_dtype, elements, comparisons)?;
            self.run_plan(plan).await?;
        }
        if !in_range && seen.len() != outputs {
            return sim_error!("empty nested min/max window");
        }
        Ok(())
    }

    fn build_extremum_chunk_plan(
        &self,
        value: &ExprSpec,
        result_dtype: DataType,
        elements: usize,
        comparisons: usize,
    ) -> Result<ExecutionPlan, SimError> {
        let work = build_scalar_expression_work(value, self.region, result_dtype, elements)?.0;
        let mut plan = self.runner.compute_model.lower(&work)?;
        plan.stages.extend(
            self.build_comparison_plan(result_dtype, comparisons)?
                .stages,
        );
        Ok(plan)
    }

    async fn run_local_extremum(&self, tensor: &str, over: &[String]) -> SimResult {
        let outputs = self.count_output_elements("local reduction output count overflows")?;
        for (chunk_index, chunk) in self.tile.reduction_chunks_for(over).enumerate() {
            let reduction_len = chunk.values().try_fold(1usize, |count, range| {
                count
                    .checked_mul(range.len())
                    .ok_or_else(|| SimError("local reduction size overflows".into()))
            })?;
            let comparisons = outputs
                .checked_mul(reduction_len - usize::from(chunk_index == 0))
                .ok_or_else(|| SimError("local reduction comparisons overflow".into()))?;
            if comparisons != 0 {
                let dtype = self.region.value(tensor).unwrap().dtype;
                let plan = self.runner.compute_model.lower(&ComputeWork::operation(
                    "compare",
                    vec![dtype, dtype],
                    comparisons,
                ))?;
                self.run_plan(plan).await?;
            }
        }
        Ok(())
    }

    async fn run_window_extremum(
        &self,
        tensor: &str,
        indices: &[IndexSpec],
        over: &[String],
    ) -> SimResult {
        let input = self.boundary_input(tensor)?;
        let in_range = window_indices_are_in_range(indices, self.tile, self.region, input)?;
        let mut seen = BTreeSet::<Vec<usize>>::new();
        for (chunk_index, chunk) in self.tile.reduction_chunks_for(over).enumerate() {
            self.load_inputs(&chunk, &format!("window chunk {chunk_index} read"))
                .await?;
            let (_, comparisons) = count_window_chunk_operations(
                &[(indices, input)],
                &self.assignment.axes,
                over,
                self.tile,
                self.region,
                &chunk,
                chunk_index == 0,
                in_range,
                &mut seen,
            )?;
            // Initialization consumes the chunk even when no comparison is
            // needed.
            let plan =
                self.build_comparison_plan(self.region.value(tensor).unwrap().dtype, comparisons)?;
            self.run_plan(plan).await?;
        }
        if !in_range {
            let outputs = self.count_output_elements("max output count overflows")?;
            if seen.len() != outputs {
                return sim_error!("empty min/max window");
            }
        }
        Ok(())
    }

    fn boundary_input(&self, tensor: &str) -> Result<&TensorView, SimError> {
        let index = self
            .config
            .program
            .inputs
            .iter()
            .position(|name| name == tensor)
            .ok_or_else(|| SimError(format!("max source {tensor} is not a boundary input")))?;
        Ok(&self.config.inputs[index])
    }

    fn build_comparison_plan(
        &self,
        dtype: DataType,
        comparisons: usize,
    ) -> Result<ExecutionPlan, SimError> {
        if comparisons == 0 {
            return Ok(ExecutionPlan { stages: vec![] });
        }
        self.runner.compute_model.lower(&ComputeWork::operation(
            "compare",
            vec![dtype, dtype],
            comparisons,
        ))
    }

    async fn run_reduction(&self) -> SimResult {
        let over = match &self.assignment.expr {
            ExprSpec::Reduce { over, .. } | ExprSpec::MatrixMac { over, .. } => over,
            _ => unreachable!("sum and matrix reduction dispatch"),
        };
        let outputs = self.count_output_elements("tile output count overflows")?;
        let excluded = self.collect_excluded_inputs(&self.assignment.expr);
        let in_range = excluded
            .iter()
            .try_fold(true, |present, (indices, input)| {
                Ok::<bool, SimError>(
                    present && window_indices_are_in_range(indices, self.tile, self.region, input)?,
                )
            })?;
        let mut seen = BTreeSet::new();
        for (chunk_index, chunk) in self.tile.reduction_chunks_for(over).enumerate() {
            let mut intervals = self.tile.output_intervals.clone();
            intervals.extend(chunk.clone());
            let (elements, additions) = count_window_chunk_operations(
                &excluded,
                &self.assignment.axes,
                over,
                self.tile,
                self.region,
                &chunk,
                chunk_index == 0,
                in_range,
                &mut seen,
            )?;
            let matrix_plan = self.build_matrix_chunk_plan(&intervals, elements)?;
            self.load_inputs(&chunk, &format!("chunk {chunk_index} read"))
                .await?;
            let plan = self.build_reduction_chunk_plan(matrix_plan, elements, additions)?;
            self.run_plan(plan).await?;
        }
        self.convert_matrix_output(outputs).await
    }

    async fn convert_matrix_output(&self, outputs: usize) -> SimResult {
        if let ExprSpec::MatrixMac { accumulator, .. } = &self.assignment.expr {
            let output = self
                .region
                .value(&self.assignment.primary_result())
                .unwrap()
                .dtype;
            if *accumulator != output {
                let plan = self.runner.compute_model.lower(&ComputeWork::operation(
                    "convert",
                    vec![*accumulator],
                    outputs,
                ))?;
                self.run_plan(plan).await?;
            }
        }
        Ok(())
    }

    fn build_matrix_chunk_plan(
        &self,
        intervals: &BTreeMap<String, Range<usize>>,
        elements: usize,
    ) -> Result<Option<ExecutionPlan>, SimError> {
        build_matrix_work(self.assignment, self.region, intervals)?
            .map(|work| {
                if work.stages[0].operations[0].count != elements {
                    return Err(SimError(
                        "matrix shape does not match chunk operations".into(),
                    ));
                }
                self.runner.compute_model.lower(&work)
            })
            .transpose()
    }

    fn build_reduction_chunk_plan(
        &self,
        matrix_plan: Option<ExecutionPlan>,
        elements: usize,
        additions: usize,
    ) -> Result<ExecutionPlan, SimError> {
        if let Some(plan) = matrix_plan {
            return Ok(plan);
        }
        let ExprSpec::Reduce { value, .. } = &self.assignment.expr else {
            unreachable!("matrix reduction already has a plan")
        };
        let dtype = self
            .region
            .value(&self.assignment.primary_result())
            .unwrap()
            .dtype;
        let mut plan = match &**value {
            ExprSpec::Read { .. } => ExecutionPlan { stages: vec![] },
            ExprSpec::Call { .. } => {
                let work = build_scalar_expression_work(value, self.region, dtype, elements)?.0;
                self.runner.compute_model.lower(&work)?
            }
            _ => unreachable!("validated scalar reduction body"),
        };
        if additions != 0 {
            let extra = self.runner.compute_model.lower(&ComputeWork::operation(
                "add",
                vec![dtype, dtype],
                additions,
            ))?;
            plan.stages.extend(extra.stages);
        }
        Ok(plan)
    }

    async fn load_inputs(&self, chunk: &BTreeMap<String, Range<usize>>, label: &str) -> SimResult {
        for read in &self.input_reads {
            self.runner
                .load_input(
                    read.plan_access(self.tile, self.region, chunk)?,
                    &format!("{} {} {label}", self.config.id, read.name),
                    self.group,
                )
                .await?;
        }
        Ok(())
    }

    async fn run_plan(&self, plan: ExecutionPlan) -> SimResult {
        for stage in plan.stages {
            try_join_all(stage.into_iter().map(|batch| self.run_compute_batch(batch))).await?;
        }
        Ok(())
    }

    async fn run_compute_batch(&self, batch: ComputeBatch) -> SimResult {
        let runner = self.runner;
        let assignment = self.assignment.primary_result();
        let (reservation, ticks) = self.reserve_compute_batch(&batch).await?;
        runner.clock.wait_phase(END).await;
        runner
            .clock
            .tick_now()
            .tick()
            .checked_add(ticks)
            .ok_or_else(|| {
                SimError(format!(
                    "{} {assignment} compute completion tick overflows",
                    self.config.id
                ))
            })?;
        debug!(runner.entity; "{} tile {:?} assignment {assignment} begin", self.config.id, self.tile.output_intervals);
        let activity = ActivityLanes::begin_in_group(
            &runner.activity_lanes.compute[reservation.execution_unit().0],
            &format!("{} {assignment} compute", self.config.id),
            self.group,
        );
        if let Some(monitor) = &runner.flop_monitor {
            monitor.record_interval(ticks, batch.machine_ops.flop_equivalents() as f64);
        }
        runner.clock.wait_ticks(ticks)?.await;
        debug!(runner.entity; "{} tile {:?} assignment {assignment} end", self.config.id, self.tile.output_intervals);
        drop(activity);
        drop(reservation);
        let mut stats = runner.stats.borrow_mut();
        stats.machine_ops = stats.machine_ops.clone().checked_add(batch.machine_ops)?;
        Ok(())
    }

    async fn reserve_compute_batch(
        &self,
        batch: &ComputeBatch,
    ) -> Result<(ComputeReservation, u64), SimError> {
        let runner = self.runner;
        let assignment = self.assignment.primary_result();
        runner.clock.wait_phase(END).await;
        let waiting = ActivityLanes::begin_in_group(
            &runner.activity_lanes.compute_wait,
            &format!("{} {assignment} compute wait", self.config.id),
            self.group,
        );
        let eligible = batch
            .alternatives
            .iter()
            .map(|(unit, _)| *unit)
            .collect::<Vec<_>>();
        let reservation = runner
            .compute_scheduler
            .acquire_any_with_accumulator(&eligible, batch.matrix_accumulator_bytes)
            .await?;
        let unit = reservation.execution_unit();
        let ticks = batch
            .alternatives
            .iter()
            .find(|(candidate, _)| *candidate == unit)
            .ok_or_else(|| SimError("selected execution unit missing from batch".into()))?
            .1;
        drop(waiting);
        Ok((reservation, ticks))
    }

    fn count_output_elements(&self, error: &str) -> Result<usize, SimError> {
        self.assignment.axes.iter().try_fold(1usize, |count, axis| {
            count
                .checked_mul(
                    self.tile
                        .output_intervals
                        .get(axis)
                        .map_or(self.region.extent(axis).unwrap(), Range::len),
                )
                .ok_or_else(|| SimError(error.into()))
        })
    }

    fn find_primary_excluded_input(
        &self,
        value: &'a ExprSpec,
    ) -> Option<(&'a [IndexSpec], &'a TensorView)> {
        let expr = value.iter_reads().find(|expr| {
            matches!(
                expr,
                ExprSpec::Read {
                    bounds: BoundsMode::Exclude,
                    ..
                }
            )
        })?;
        self.resolve_excluded_input(expr)
    }

    fn collect_excluded_inputs(
        &self,
        expr: &'a ExprSpec,
    ) -> Vec<(&'a [IndexSpec], &'a TensorView)> {
        expr.iter_reads()
            .filter_map(|read| self.resolve_excluded_input(read))
            .collect()
    }

    fn resolve_excluded_input(
        &self,
        expr: &'a ExprSpec,
    ) -> Option<(&'a [IndexSpec], &'a TensorView)> {
        let ExprSpec::Read {
            tensor,
            indices,
            bounds,
        } = expr
        else {
            unreachable!()
        };
        if *bounds != BoundsMode::Exclude {
            return None;
        }
        self.config
            .program
            .inputs
            .iter()
            .position(|input| input == tensor)
            .map(|index| (indices.as_slice(), &self.config.inputs[index]))
    }
}

fn count_extremum_operations(
    outputs: usize,
    reduction_len: usize,
    first_chunk: bool,
) -> Result<(usize, usize), SimError> {
    let elements = outputs
        .checked_mul(reduction_len)
        .ok_or_else(|| SimError("extremum body count overflows".into()))?;
    let comparisons = outputs
        .checked_mul(reduction_len - usize::from(first_chunk))
        .ok_or_else(|| SimError("extremum comparison count overflows".into()))?;
    Ok((elements, comparisons))
}
