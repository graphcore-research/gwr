// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::oneshot::channel;
use futures::future::{Either, select};
use futures::stream::FuturesUnordered;
use gwr_components::capacity_allocator::CapacityAllocator;
use gwr_engine::sim_error;
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::entity::{Entity, EntityGroup};

use self::access::InputAccess;
use self::helpers::{
    build_assignment_dependencies, build_scalar_expression_work, eval_index, find_first_read_dtype,
    find_read_indices, slice_axes, slice_view,
};
use self::matrix_shape::build_matrix_work;
use self::schedule::{estimate_scoped_peak_sram, visit_groups};
use self::tile::estimate_tile_sram;
use self::window::{
    count_window_chunk_operations, max_window_chunk_buffer_bytes, plan_window_chunk_access_ranges,
    window_chunk_packed_bytes, window_indices_are_in_range,
};
use crate::processing_element::activity::{ActivityLanes, ProcessingElementActivityLanes};
use crate::processing_element::compute::{ComputeModel, ComputeScheduler, ComputeWork};
use crate::processing_element::flop_monitor::FlopMonitor;
use crate::processing_element::load_store_unit::LoadStoreUnit;
use crate::processing_element::program::schedule::plan_schedule;
use crate::processing_element::program::syntax::{
    AssignmentSpec, BoundsMode, ExprSpec, IndexSpec, ProgramSpec, ReductionOp, TileSpec,
};
use crate::processing_element::program::tile::{RegionTile, TileSchedule, plan_tiles};
use crate::processing_element::program::validate::{
    RegionBindings, ValidatedRegion, validate_program,
};
use crate::processing_element::task::{ProgramTaskConfig, Task};
use crate::processing_element::tensors::TensorView;
use crate::processing_element::{Dispatcher, ProcessingElementStats};

mod access;
mod assignment;
mod helpers;

mod matrix_shape;
pub(super) mod schedule;

mod tile;
mod traffic;
mod window;

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
    pub(super) stats: Rc<RefCell<ProcessingElementStats>>,
    pub(super) activity_lanes: Rc<ProcessingElementActivityLanes>,
    pub(super) flop_monitor: Option<Rc<FlopMonitor>>,
}

impl TaskRunner {
    pub(super) async fn run(self, task_idx: usize) -> SimResult {
        let task = self.dispatcher.task_by_id(task_idx)?;
        match task {
            Task::ProgramTask { config } => self.run_program(task_idx, &config).await,
            Task::SyncTask { .. } => todo!(),
        }
    }

    async fn run_program(&self, task_idx: usize, config: &ProgramTaskConfig) -> SimResult {
        self.lsu.validate_program_views(config)?;
        let region = validate_program(
            &config.program,
            &RegionBindings {
                inputs: config.inputs.clone(),
                outputs: config.outputs.clone(),
            },
        )?;
        if let Some(schedule) = plan_schedule(&region, config.tiling.as_ref())? {
            self.run_scoped_program(config, &region, &schedule).await?;
        } else {
            self.run_flat_program(config, &region).await?;
        }
        self.dispatcher.set_task_completed(task_idx)
    }

    async fn run_flat_program(
        &self,
        config: &ProgramTaskConfig,
        region: &ValidatedRegion,
    ) -> SimResult {
        let schedule = plan_tiles(region, config.tiling.as_ref())?;
        validate_program_compute_support(&self.compute_model, region, &schedule)?;
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
                active.push(self.run_program_tile(config, region, tile, sender));
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
    if let Some(schedule) = plan_schedule(&region, Some(tiling))? {
        return estimate_scoped_peak_sram(&config, &region, &schedule, None);
    }
    let schedule = plan_tiles(&region, Some(tiling))?;
    estimate_flat_schedule_peak_sram(&config, &region, &schedule)
}

fn estimate_flat_schedule_peak_sram(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    schedule: &TileSchedule,
) -> Result<usize, SimError> {
    // Packed elementwise/matrix slices reserve logical packed storage, whose
    // size is independent of physical bit offsets. Byte-oriented affine
    // windows with nonnegative offsets can only shrink at the trailing edge;
    // their first full tile is maximal. Preserve exhaustive checking for
    // leading padding and packed reduction-window byte alignment.
    let packed = config.inputs.iter().any(|view| view.dtype().num_bits() < 8);
    let inspect_all = config.program.assignments.iter().any(|assignment| {
        uses_leading_padding(&assignment.expr)
            || (packed && uses_reduction_window(&assignment.expr))
    });
    if inspect_all {
        schedule.tiles().try_fold(0, |peak, tile| {
            Ok(peak.max(estimate_tile_sram(config, region, &tile)?))
        })
    } else {
        let first = schedule
            .tiles()
            .next()
            .ok_or_else(|| SimError("program has no tiles".into()))?;
        estimate_tile_sram(config, region, &first)
    }
}

impl ComputeModel {
    /// Validate assignment capabilities and logical tile geometry using the
    /// same compute rules as program execution. SRAM capacity and boundary
    /// memory mappings are checked separately.
    pub fn validate_program(
        &self,
        region: &ValidatedRegion,
        tiling: Option<&TileSpec>,
    ) -> SimResult {
        if let Some(schedule) = plan_schedule(region, tiling)? {
            visit_groups(&schedule, region, tiling, &mut |indices, tile, _| {
                for &index in indices {
                    validate_assignment_compute_support(self, region, index, tile)?;
                }
                Ok(())
            })
        } else {
            let schedule = plan_tiles(region, tiling)?;
            validate_program_compute_support(self, region, &schedule)
        }
    }
}

pub(super) fn validate_program_compute_support(
    compute_model: &ComputeModel,
    region: &ValidatedRegion,
    schedule: &TileSchedule,
) -> SimResult {
    let first = schedule
        .tiles()
        .next()
        .ok_or_else(|| SimError("program has no tiles".into()))?;
    for &index in &region.assignment_order {
        validate_assignment_compute_support(compute_model, region, index, &first)?;
    }
    Ok(())
}

pub(super) fn validate_assignment_compute_support(
    compute_model: &ComputeModel,
    region: &ValidatedRegion,
    index: usize,
    first: &RegionTile,
) -> SimResult {
    let assignment = &region.program.assignments[index];
    let work = match &assignment.expr {
        ExprSpec::Read { .. } => return Ok(()),
        ExprSpec::Call { .. } => {
            build_scalar_expression_work(
                &assignment.expr,
                region,
                region.value(&assignment.primary_result()).unwrap().dtype,
                1,
            )?
            .0
        }
        ExprSpec::MatrixMac { over, .. } => {
            build_matrix_validation_work(assignment, over, region, first)?
        }
        ExprSpec::Reduce {
            op: ReductionOp::Min | ReductionOp::Max,
            over,
            value,
        } => {
            return validate_extremum_compute_support(
                compute_model,
                assignment,
                value,
                over,
                region,
            );
        }
        ExprSpec::Reduce {
            op: ReductionOp::Sum,
            over,
            value,
        } => {
            return validate_sum_compute_support(compute_model, assignment, value, over, region);
        }
    };
    validate_matrix_output_conversion(compute_model, assignment, region)?;
    validate_assignment_work(compute_model, assignment, &work)
}

fn build_matrix_validation_work(
    assignment: &AssignmentSpec,
    over: &[String],
    region: &ValidatedRegion,
    first: &RegionTile,
) -> Result<ComputeWork, SimError> {
    let mut intervals = first.output_intervals.clone();
    let chunk = first
        .reduction_chunks_for(over)
        .next()
        .ok_or_else(|| SimError("matrix has no reduction chunks".into()))?;
    intervals.extend(chunk);
    build_matrix_work(assignment, region, &intervals)?
        .ok_or_else(|| SimError("matrix assignment has no logical geometry".into()))
}

fn validate_extremum_compute_support(
    model: &ComputeModel,
    assignment: &AssignmentSpec,
    value: &ExprSpec,
    over: &[String],
    region: &ValidatedRegion,
) -> SimResult {
    let size = reduction_length(over, region, "min/max reduction size overflows")?;
    let result_dtype = if matches!(value, ExprSpec::Call { .. }) {
        assignment.output.as_ref().map_or_else(
            || find_first_read_dtype(value, region),
            |name| region.value(name).unwrap().dtype,
        )
    } else {
        let ExprSpec::Read { tensor, .. } = value else {
            return sim_error!("unsupported expression in compute validation");
        };
        region
            .value(tensor)
            .ok_or_else(|| SimError(format!("unknown tensor {tensor}")))?
            .dtype
    };
    let body = build_scalar_expression_work(value, region, result_dtype, 1)?.0;
    model.validate_work(&body)?;
    if size > 1 {
        let comparisons = ComputeWork::operation("compare", vec![result_dtype, result_dtype], 1);
        validate_assignment_work(model, assignment, &comparisons)?;
    }
    Ok(())
}

fn validate_sum_compute_support(
    model: &ComputeModel,
    assignment: &AssignmentSpec,
    value: &ExprSpec,
    over: &[String],
    region: &ValidatedRegion,
) -> SimResult {
    let size = reduction_length(over, region, "product reduction size overflows")?;
    let output = region.value(&assignment.primary_result()).unwrap().dtype;
    let body = build_scalar_expression_work(value, region, output, 1)?.0;
    model.validate_work(&body)?;
    if size > 1 {
        let additions = ComputeWork::operation("add", vec![output, output], 1);
        validate_assignment_work(model, assignment, &additions)?;
    }
    Ok(())
}

fn validate_assignment_work(
    model: &ComputeModel,
    assignment: &AssignmentSpec,
    work: &ComputeWork,
) -> SimResult {
    model.validate_work(work).map_err(|error| {
        SimError(format!(
            "assignment {}: {}",
            assignment.primary_result(),
            error.0
        ))
    })
}

fn reduction_length(
    over: &[String],
    region: &ValidatedRegion,
    overflow: &str,
) -> Result<usize, SimError> {
    over.iter().try_fold(1usize, |count, axis| {
        count
            .checked_mul(region.extent(axis).unwrap())
            .ok_or_else(|| SimError(overflow.into()))
    })
}

fn validate_matrix_output_conversion(
    model: &ComputeModel,
    assignment: &AssignmentSpec,
    region: &ValidatedRegion,
) -> SimResult {
    if let ExprSpec::MatrixMac { accumulator, .. } = &assignment.expr {
        let output = region.value(&assignment.primary_result()).unwrap().dtype;
        if *accumulator != output {
            model.lower(&ComputeWork::operation("convert", vec![*accumulator], 1))?;
        }
    }
    Ok(())
}

fn uses_leading_padding(expr: &ExprSpec) -> bool {
    match expr {
        ExprSpec::Read {
            indices,
            bounds: BoundsMode::Exclude,
            ..
        } => indices
            .iter()
            .any(|index| matches!(index, IndexSpec::Affine { affine } if affine.offset < 0)),
        ExprSpec::Call { args, .. } | ExprSpec::MatrixMac { args, .. } => {
            args.iter().any(uses_leading_padding)
        }
        ExprSpec::Reduce { value, .. } => uses_leading_padding(value),
        ExprSpec::Read { .. } => false,
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

#[cfg(test)]
mod matrix_test_fixture;

#[cfg(test)]
mod matrix_accumulator_tests;
#[cfg(test)]
mod matrix_shape_tests;
