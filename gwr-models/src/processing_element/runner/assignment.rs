// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use futures::future::try_join_all;
use gwr_engine::time::clock::phase::END;
use gwr_engine::types::{SimError, SimResult};
use gwr_track::debug;
use gwr_track::entity::EntityGroup;

use super::TaskRunner;
use crate::processing_element::activity::ActivityLanes;
use crate::processing_element::compute::{ComputeBatch, ComputeReservation, ExecutionPlan};
use crate::processing_element::program::analysis::access::{
    InputRead, collect_assignment_input_reads,
};
use crate::processing_element::program::analysis::invocation::AssignmentInvocations;
use crate::processing_element::program::executable::Assignment;
use crate::processing_element::program::prepared::GroupPlan;
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ProgramFacts;
use crate::processing_element::task::PreparedTask;

impl TaskRunner {
    pub(super) async fn run_program_assignment(
        &self,
        task: &PreparedTask,
        plan: GroupPlan<'_>,
        tile: &RegionTile,
        group: &Rc<EntityGroup>,
        assignment_index: usize,
    ) -> SimResult {
        AssignmentExecution {
            runner: self,
            task,
            region: plan.region(),
            tile,
            group,
            assignment: &plan.region().program.assignments[assignment_index],
            assignment_index,
            input_reads: collect_assignment_input_reads(plan, assignment_index),
        }
        .run()
        .await
    }
}

struct AssignmentExecution<'a> {
    runner: &'a TaskRunner,
    task: &'a PreparedTask,
    region: &'a ProgramFacts,
    tile: &'a RegionTile,
    group: &'a Rc<EntityGroup>,
    assignment: &'a Assignment,
    input_reads: Vec<InputRead<'a>>,
    assignment_index: usize,
}

impl AssignmentExecution<'_> {
    async fn run(&self) -> SimResult {
        for invocation in AssignmentInvocations::new(self.region, self.assignment_index, self.tile)?
        {
            let invocation = invocation?;
            if let Some(label) = invocation.read_label {
                self.load_inputs(&invocation.chunk, &label).await?;
            }
            self.run_plan(self.task.lower(&invocation.work)?).await?;
        }
        Ok(())
    }

    async fn load_inputs(&self, chunk: &BTreeMap<String, Range<usize>>, label: &str) -> SimResult {
        for read in &self.input_reads {
            self.runner
                .load_input(
                    read.plan_access(self.tile, self.region, chunk)?,
                    &format!("{} {} {label}", self.task.id(), read.name),
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
        let reservation = self.reserve_compute_batch(&batch).await?;
        let ticks = reservation.ticks();
        runner.clock.wait_phase(END).await;
        runner
            .clock
            .tick_now()
            .tick()
            .checked_add(ticks)
            .ok_or_else(|| {
                SimError(format!(
                    "{} {assignment} compute completion tick overflows",
                    self.task.id()
                ))
            })?;
        debug!(runner.entity; "{} tile {:?} assignment {assignment} begin", self.task.id(), self.tile.output_intervals);
        let activity = ActivityLanes::begin_in_group(
            &runner.activity_lanes.compute[reservation.execution_unit().0],
            &format!("{} {assignment} compute", self.task.id()),
            self.group,
        );
        if let Some(monitor) = &runner.flop_monitor {
            monitor.record_interval(ticks, batch.machine_ops.flop_equivalents() as f64);
        }
        runner.clock.wait_ticks(ticks)?.await;
        debug!(runner.entity; "{} tile {:?} assignment {assignment} end", self.task.id(), self.tile.output_intervals);
        drop(activity);
        drop(reservation);
        let mut stats = runner.stats.borrow_mut();
        stats.machine_ops = stats.machine_ops.clone().checked_add(batch.machine_ops)?;
        Ok(())
    }

    async fn reserve_compute_batch(
        &self,
        batch: &ComputeBatch,
    ) -> Result<ComputeReservation, SimError> {
        let runner = self.runner;
        let assignment = self.assignment.primary_result();
        runner.clock.wait_phase(END).await;
        let waiting = ActivityLanes::begin_in_group(
            &runner.activity_lanes.compute_wait,
            &format!("{} {assignment} compute wait", self.task.id()),
            self.group,
        );
        let reservation = runner
            .compute_scheduler
            .acquire(&batch.alternatives, batch.matrix_accumulator_bytes)
            .await?;
        drop(waiting);
        Ok(reservation)
    }
}
