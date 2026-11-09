// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::oneshot;
use futures::future::{Either, select};
use futures::stream::FuturesUnordered;
use gwr_components::capacity_allocator::CapacityAllocator;
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::entity::{Entity, EntityGroup};

use crate::processing_element::activity::{ActivityLanes, ProcessingElementActivityLanes};
use crate::processing_element::boundary_payload::{
    BoundaryPayloadDirection, BoundaryPayloadService,
};
use crate::processing_element::compute::{ComputeModel, ComputeScheduler, ComputeWork};
use crate::processing_element::flop_monitor::FlopMonitor;
use crate::processing_element::load_store_unit::LoadStoreUnit;
use crate::processing_element::program::syntax::{
    ExprSpec, IndexSpec, ProgramSpec, ReductionOp, TileSpec,
};
use crate::processing_element::program::tile::plan_tiles;
use crate::processing_element::program::validate::{
    RegionBindings, ValidatedRegion, validate_program,
};
use crate::processing_element::task::{ProgramTaskConfig, Task};
use crate::processing_element::tensors::TensorView;
use crate::processing_element::{Dispatcher, ProcessingElementStats};

mod assignment;
mod block_pipeline;
#[cfg(test)]
mod block_pipeline_tests;
#[cfg(test)]
mod boundary_tests;
mod helpers;
#[cfg(test)]
mod indexed_metadata_tests;
#[cfg(test)]
mod local_payload_tests;
#[cfg(test)]
mod matrix_accumulator_tests;
mod matrix_shape;
#[cfg(test)]
mod matrix_shape_tests;
pub(super) mod program_pipeline;
#[cfg(test)]
mod program_pipeline_tests;
pub mod streamed_matrix;
mod tile;
mod traffic;
mod window;

use helpers::{
    align8, assignment_dependencies, collect_read_names, eval_index, find_read, slice_axes,
    slice_view,
};
use window::{
    count_window_chunk_operations, max_window_chunk_buffer_bytes, window_all_in_range,
    window_chunk_access_ranges, window_input_view,
};

#[derive(Clone)]
pub(super) struct TaskRunner {
    pub(super) entity: Rc<Entity>,
    pub(super) clock: Clock,
    pub(super) dispatcher: Dispatcher,
    pub(super) lsu: Rc<LoadStoreUnit>,
    pub(super) compute_model: Rc<ComputeModel>,
    pub(super) compute_scheduler: ComputeScheduler,
    pub(super) sram: CapacityAllocator,
    pub(super) sram_bytes: usize,
    pub(super) block_execution: Option<Rc<super::block_runtime::BlockExecutionResources>>,
    pub(super) boundary_payload: Option<Rc<BoundaryPayloadService>>,
    pub(super) local_payload: Option<Rc<BoundaryPayloadService>>,
    pub(super) stats: Rc<RefCell<ProcessingElementStats>>,
    pub(super) activity_lanes: Rc<ProcessingElementActivityLanes>,
    pub(super) flop_monitor: Option<Rc<FlopMonitor>>,
}

impl TaskRunner {
    /// Caller-convention packed payload at the program LSU/tile boundary.
    /// Reads stage returned bytes into local storage; writes stage local bytes
    /// before external issue. No expression/alias/DMA traffic is implied.
    async fn staged_access_ranges(
        &self,
        access_type: AccessType,
        ranges: impl IntoIterator<Item = Range<u128>>,
        payload_bytes: usize,
        activity_lanes: &Rc<RefCell<ActivityLanes>>,
        activity_name: &str,
        group: &Rc<EntityGroup>,
    ) -> SimResult {
        if let Some(service) = &self.boundary_payload {
            match access_type {
                AccessType::ReadRequest => {
                    self.lsu
                        .access_ranges(access_type, ranges, activity_lanes, activity_name, group)
                        .await?;
                    // External payload is real even if local admission or its
                    // wait fails.
                    self.record_read_bytes(payload_bytes)?;
                    service
                        .reserve(BoundaryPayloadDirection::Write, payload_bytes)?
                        .complete()
                        .await
                }
                AccessType::WriteNonPostedRequest => {
                    service
                        .reserve(BoundaryPayloadDirection::Read, payload_bytes)?
                        .complete()
                        .await?;
                    self.lsu
                        .access_ranges(access_type, ranges, activity_lanes, activity_name, group)
                        .await
                }
                _ => Err(SimError("unsupported program boundary access type".into())),
            }
        } else {
            self.lsu
                .access_ranges(access_type, ranges, activity_lanes, activity_name, group)
                .await?;
            if matches!(access_type, AccessType::ReadRequest) {
                self.record_read_bytes(payload_bytes)?;
            }
            Ok(())
        }
    }
    pub(super) async fn run(self, task_idx: usize) -> SimResult {
        let task = self.dispatcher.task_by_id(task_idx)?;
        match task {
            Task::StreamedMatrix { config } => {
                config.validate_runner(&self.entity)?;
                let scratch = self.run_stream(config.config()).await?;
                config.publish(scratch)?;
                self.dispatcher.set_task_completed(task_idx)
            }
            Task::ProgramTask { config } => self.run_program(task_idx, &config).await,
            Task::AtomicControlJoin { config } => {
                config.execute()?;
                self.dispatcher.set_task_completed(task_idx)
            }
            Task::AtomicEffect { config } => {
                config.execute().await?;
                // execute resolves only after actual publisher PortPut ACK and
                // owner retirement. Exactly one completion; no empty program.
                self.dispatcher.set_task_completed(task_idx)
            }
            Task::SyncTask { .. } => todo!(),
            Task::TransferTask { .. } => unreachable!("PE handles transfer tasks"),
        }
    }

    async fn run_program(&self, task_idx: usize, config: &ProgramTaskConfig) -> SimResult {
        self.lsu.validate_program_views(config)?;
        if config.pipeline.is_some() {
            for view in config.inputs.iter().chain(&config.outputs) {
                self.lsu.validate_aligned_view(view)?;
            }
            if self.boundary_payload.is_some() {
                return Err(SimError("pipeline with boundary_payload_service is unsupported; use block_execution local_memory_service".into()));
            }
            let resources = self
                .block_execution
                .as_ref()
                .ok_or_else(|| SimError("pipeline requires block_execution support".into()))?;
            let plan = program_pipeline::compile(config, &self.compute_model, &resources.config)?;
            if self.local_payload.is_some() {
                return sim_error!("pipeline with local_payload_service is unsupported");
            }
            match plan {
                program_pipeline::PipelinePlan::Matrix(plan) => {
                    self.run_block_pipeline(config, plan).await?
                }
                program_pipeline::PipelinePlan::Program(plan) => {
                    self.run_program_pipeline(config, *plan).await?
                }
            }
            return self.dispatcher.set_task_completed(task_idx);
        }
        super::local_payload::preflight(config, self.local_payload.is_some(), self.sram_bytes)?;
        let region = validate_program(
            &config.program,
            &RegionBindings {
                inputs: config.inputs.clone(),
                outputs: config.outputs.clone(),
            },
        )?;
        let schedule = plan_tiles(&region, config.tiling.as_ref())?;
        preflight_program(&self.compute_model, &region, &schedule)?;
        // At most one tile waits for SRAM admission. Already admitted tiles
        // continue running and may release capacity while that wait is live.
        let mut tiles = schedule.tiles();
        let mut active = FuturesUnordered::new();
        let mut pending = None;
        loop {
            if pending.is_none()
                && let Some(tile) = tiles.next()
            {
                let (sender, receiver) = oneshot::channel();
                active.push(self.run_program_tile(config, &region, tile, sender));
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
        self.dispatcher.set_task_completed(task_idx)
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

/// Maximum SRAM required by any tile of a program. Direct, byte-aligned
/// accesses only need the first (largest) tile; other layouts are inspected
/// tile by tile because their physical access size may vary with offsets.
pub fn estimate_peak_tile_sram(
    program: &ProgramSpec,
    inputs: &[TensorView],
    outputs: &[TensorView],
    tiling: &TileSpec,
) -> Result<usize, SimError> {
    let config = ProgramTaskConfig {
        pipeline: None,
        id: "SRAM estimate".into(),
        program: program.clone(),
        tiling: Some(tiling.clone()),
        inputs: inputs.to_vec(),
        outputs: outputs.to_vec(),
    };
    let region = validate_program(
        program,
        &RegionBindings {
            inputs: inputs.to_vec(),
            outputs: outputs.to_vec(),
        },
    )?;
    let schedule = plan_tiles(&region, Some(tiling))?;
    // Packed elementwise/matrix slices reserve logical packed storage, whose
    // size is independent of physical bit offsets. Byte-oriented affine
    // windows with nonnegative offsets can only shrink at the trailing edge;
    // their first full tile is maximal. Preserve exhaustive checking for
    // leading padding and packed reduction-window byte alignment.
    let packed = inputs.iter().any(|view| view.dtype().num_bits() < 8);
    let inspect_all = program.assignments.iter().any(|assignment| {
        uses_leading_padding(&assignment.expr)
            || (packed && uses_reduction_window(&assignment.expr))
    });
    if inspect_all {
        schedule.tiles().try_fold(0, |peak, tile| {
            Ok(peak.max(tile::estimate_tile_sram(&config, &region, &tile)?))
        })
    } else {
        let first = schedule
            .tiles()
            .next()
            .ok_or_else(|| SimError("program has no tiles".into()))?;
        tile::estimate_tile_sram(&config, &region, &first)
    }
}

pub(super) fn preflight_program(
    compute_model: &ComputeModel,
    region: &ValidatedRegion,
    schedule: &crate::processing_element::program::tile::TileSchedule,
) -> SimResult {
    for index in &region.assignment_order {
        let assignment = &region.program.assignments[*index];
        let dtype = |expr: &ExprSpec| -> Result<_, SimError> {
            let ExprSpec::Read { tensor, .. } = expr else {
                return sim_error!("unsupported expression in compute preflight");
            };
            Ok(region
                .value(tensor)
                .ok_or_else(|| SimError(format!("unknown tensor {tensor}")))?
                .dtype)
        };
        let work = match &assignment.expr {
            ExprSpec::Read { .. } => continue,
            ExprSpec::Call { .. } => {
                helpers::expression_work(
                    &assignment.expr,
                    region,
                    region.value(&assignment.primary_result()).unwrap().dtype,
                    1,
                )?
                .0
            }
            ExprSpec::MatrixMac {
                args,
                execution,
                over,
                ..
            } => {
                let first = schedule
                    .tiles()
                    .next()
                    .ok_or_else(|| SimError("program has no tiles".into()))?;
                let mut intervals = first.output_intervals.clone();
                let chunk = first
                    .reduction_chunks_for(over)
                    .next()
                    .ok_or_else(|| SimError("matrix has no reduction chunks".into()))?;
                intervals.extend(chunk);
                let plain = ComputeWork::operation(
                    "matrix_mac",
                    args.iter().map(dtype).collect::<Result<Vec<_>, _>>()?,
                    1,
                )
                .with_execution(execution.clone());
                matrix_shape::work(compute_model, assignment, region, &intervals)?.unwrap_or(plain)
            }
            ExprSpec::Reduce {
                op: ReductionOp::Min | ReductionOp::Max,
                over,
                value,
            } => {
                let size = over.iter().try_fold(1usize, |count, axis| {
                    count
                        .checked_mul(region.extent(axis).unwrap())
                        .ok_or_else(|| SimError("min/max reduction size overflows".into()))
                })?;
                let result_dtype = if matches!(&**value, ExprSpec::Call { .. }) {
                    assignment.output.as_ref().map_or_else(
                        || helpers::expression_read_dtype(value, region),
                        |name| region.value(name).unwrap().dtype,
                    )
                } else {
                    dtype(value)?
                };
                compute_model
                    .lower(&helpers::expression_work(value, region, result_dtype, 1)?.0)?;
                if size <= 1 {
                    continue;
                }
                ComputeWork::operation("compare", vec![result_dtype, result_dtype], 1)
            }
            ExprSpec::Reduce {
                op: ReductionOp::Sum,
                over,
                value,
            } => {
                let reduction_len = over.iter().try_fold(1usize, |count, axis| {
                    count
                        .checked_mul(region.extent(axis).unwrap())
                        .ok_or_else(|| SimError("product reduction size overflows".into()))
                })?;
                let output = region.value(&assignment.primary_result()).unwrap().dtype;
                compute_model.lower(&helpers::expression_work(value, region, output, 1)?.0)?;
                if reduction_len <= 1 {
                    continue;
                }
                let output = region.value(&assignment.primary_result()).unwrap().dtype;
                ComputeWork::operation("add", vec![output, output], 1)
            }
        };
        if let ExprSpec::MatrixMac {
            execution: Some(mode),
            ..
        } = &assignment.expr
        {
            let output = region.value(&assignment.primary_result()).unwrap().dtype;
            if mode.accumulator != output {
                compute_model.lower(&ComputeWork::operation(
                    "convert",
                    vec![mode.accumulator],
                    1,
                ))?;
            }
        }
        compute_model.lower(&work).map_err(|error| {
            SimError(format!(
                "assignment {}: {}",
                assignment.primary_result(),
                error.0
            ))
        })?;
    }
    Ok(())
}

fn uses_leading_padding(expr: &ExprSpec) -> bool {
    match expr {
        ExprSpec::Read { indices, bounds: crate::processing_element::program::syntax::BoundsMode::Exclude, .. } =>
            indices.iter().any(|index| matches!(index, crate::processing_element::program::syntax::IndexSpec::Affine { affine } if affine.offset < 0)),
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => args.iter().any(uses_leading_padding),
        ExprSpec::Reduce { value, .. } => uses_leading_padding(value),
        _ => false,
    }
}

fn uses_reduction_window(expr: &ExprSpec) -> bool {
    match expr {
        ExprSpec::Reduce {
            op: ReductionOp::Min | ReductionOp::Max,
            ..
        } => true,
        ExprSpec::Reduce { value, .. } => uses_affine_indices(value),
        ExprSpec::Call { args, .. } => args.iter().any(uses_reduction_window),
        _ => false,
    }
}

fn uses_affine_indices(expr: &ExprSpec) -> bool {
    match expr {
        ExprSpec::Read { indices, .. } => indices
            .iter()
            .any(|index| matches!(index, IndexSpec::Affine { .. })),
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => {
            args.iter().any(uses_affine_indices)
        }
        ExprSpec::Reduce { value, .. } => uses_affine_indices(value),
    }
}
