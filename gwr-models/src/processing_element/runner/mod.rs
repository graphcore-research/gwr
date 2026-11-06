// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::oneshot::channel;
use futures::future::{Either, select};
use futures::stream::FuturesUnordered;
use gwr_components::capacity_allocator::CapacityAllocator;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::entity::{Entity, EntityGroup};

use crate::processing_element::activity::{ActivityLanes, ProcessingElementActivityLanes};
#[cfg(test)]
use crate::processing_element::compute::ComputeModel;
use crate::processing_element::compute::ComputeScheduler;
use crate::processing_element::flop_monitor::FlopMonitor;
use crate::processing_element::load_store_unit::LoadStoreUnit;
#[cfg(test)]
use crate::processing_element::program::ProgramBindings;
use crate::processing_element::program::analysis::access::InputAccess;
use crate::processing_element::program::group::GroupMetadata;
use crate::processing_element::program::prepared::Schedule;
use crate::processing_element::program::tile::TileSchedule;
#[cfg(test)]
use crate::processing_element::program::tile::plan_tiles;
#[cfg(test)]
use crate::processing_element::program::validate::validate_program;
use crate::processing_element::task::PreparedTask;
#[cfg(test)]
use crate::processing_element::task::{ProgramTaskConfig, Task};
use crate::processing_element::tensors::TensorView;
use crate::processing_element::{Dispatcher, ProcessingElementStats};

mod assignment;
mod helpers;

pub(super) mod schedule;

mod tile;

#[derive(Clone)]
pub(super) struct TaskRunner {
    pub(super) entity: Rc<Entity>,
    pub(super) clock: Clock,
    pub(super) dispatcher: Dispatcher,
    pub(super) lsu: Rc<LoadStoreUnit>,
    #[cfg(test)]
    pub(super) compute_model: Rc<ComputeModel>,
    pub(super) compute_scheduler: ComputeScheduler,
    pub(super) sram: CapacityAllocator,
    pub(super) sram_bytes: usize,
    pub(super) stats: Rc<RefCell<ProcessingElementStats>>,
    pub(super) activity_lanes: Rc<ProcessingElementActivityLanes>,
    pub(super) flop_monitor: Option<Rc<FlopMonitor>>,
}

impl TaskRunner {
    pub(super) async fn run(&self, task_idx: usize, task: Rc<PreparedTask>) -> SimResult {
        match &task.program().schedule {
            Schedule::Scoped(schedule) => {
                self.run_scoped_program(&task, schedule).await?;
            }
            Schedule::Flat { tiles, group } => {
                self.run_flat_program(&task, tiles, group).await?;
            }
        }
        self.dispatcher.set_task_completed(task_idx)
    }

    #[cfg(test)]
    async fn run_program(&self, task_idx: usize, config: &ProgramTaskConfig) -> SimResult {
        let program = Rc::new(crate::processing_element::program::PreparedProgram::new(
            &config.program,
            ProgramBindings {
                inputs: config.inputs.clone(),
                outputs: config.outputs.clone(),
            },
            config.tiling.as_ref(),
        )?);
        let task = crate::processing_element::task::PreparedTask::prepare(
            config.id.clone(),
            program,
            self.entity.clone(),
            &self.lsu,
            &self.compute_model,
            self.sram_bytes,
        )?;
        self.run(task_idx, task).await
    }

    async fn run_flat_program(
        &self,
        task: &PreparedTask,
        schedule: &TileSchedule,
        group: &GroupMetadata,
    ) -> SimResult {
        let plan = task.program().group(group);
        // At most one tile waits for SRAM admission. Already admitted tiles
        // continue running and may release capacity while that wait is live.
        let mut tiles = schedule.tiles();
        let mut active = FuturesUnordered::new();
        let mut pending = None;
        loop {
            if pending.is_none()
                && let Some(tile) = tiles.next()
            {
                let (sender, receiver) = channel();
                active.push(self.run_program_tile(task, plan, tile, sender));
                pending = Some(receiver);
            }
            if active.is_empty() {
                break;
            }
            let Some(mut receiver) = pending.take() else {
                if let Some(Err(error)) = active.next().await {
                    while active.next().await.is_some() {}
                    return Err(error);
                }
                continue;
            };
            match select(&mut receiver, active.next()).await {
                Either::Left((_admitted, _)) => {}
                Either::Right((Some(result), _)) => {
                    if let Err(error) = result {
                        while active.next().await.is_some() {}
                        return Err(error);
                    }
                    pending = Some(receiver);
                }
                Either::Right((None, _)) => break,
            }
        }
        Ok(())
    }

    async fn load_input(
        &self,
        access: InputAccess,
        activity_name: &str,
        group: &Rc<EntityGroup>,
    ) -> SimResult {
        self.access_ranges(
            AccessType::ReadRequest,
            access.ranges(),
            access.bytes(),
            &self.activity_lanes.lsu_read,
            activity_name,
            group,
        )
        .await
    }

    /// Access memory ranges and account for completed input reads.
    async fn access_ranges(
        &self,
        access_type: AccessType,
        ranges: impl IntoIterator<Item = Range<u128>>,
        payload_bytes: usize,
        activity_lanes: &Rc<RefCell<ActivityLanes>>,
        activity_name: &str,
        group: &Rc<EntityGroup>,
    ) -> SimResult {
        self.lsu
            .access_ranges(access_type, ranges, activity_lanes, activity_name, group)
            .await?;
        if matches!(access_type, AccessType::ReadRequest) {
            self.record_read_bytes(payload_bytes)?;
        }
        Ok(())
    }

    fn record_read_bytes(&self, bytes: usize) -> SimResult {
        let mut stats = self.stats.borrow_mut();
        stats.read_bytes = stats
            .read_bytes
            .checked_add(bytes)
            .ok_or_else(|| SimError("PE read traffic overflows".into()))?;
        Ok(())
    }

    fn record_write(&self, view: &TensorView) -> SimResult {
        let mut stats = self.stats.borrow_mut();
        stats.written_bytes = stats
            .written_bytes
            .checked_add(view.layout().num_access_bytes())
            .ok_or_else(|| SimError("PE write traffic overflows".into()))?;
        Ok(())
    }
}

#[cfg(test)]
mod matrix_test_fixture;

#[cfg(test)]
mod matrix_accumulator_tests;
#[cfg(test)]
mod matrix_shape_tests;
