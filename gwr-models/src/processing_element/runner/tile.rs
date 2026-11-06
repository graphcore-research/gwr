// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::oneshot::Sender;
use futures::future::LocalBoxFuture;
use futures::stream::FuturesUnordered;
use gwr_engine::sim_error;
use gwr_engine::types::{AccessType, SimError, SimResult};
use gwr_track::entity::EntityGroup;

use super::access::{
    build_tile_output_views, collect_initial_input_reads, input_requires_assignment_reads,
};
use super::helpers::find_first_read_dtype;
use super::{
    TaskRunner, build_assignment_dependencies, find_read_indices, max_window_chunk_buffer_bytes,
    slice_view, window_chunk_packed_bytes,
};
use crate::processing_element::program::executable::{Assignment, Expr, ScalarExpr};
use crate::processing_element::program::plan::{LocalAllocation, allocate_temporaries};
use crate::processing_element::program::prepared::{GroupPlan, Schedule};
use crate::processing_element::program::storage::{
    ReductionStateBinding, align_sram_slot_bytes, select_reduction_state_binding,
    select_reduction_state_binding_for_optional_output,
};
use crate::processing_element::program::syntax::{IndexSpec, ReductionOp};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ProgramFacts;
use crate::processing_element::task::PreparedTask;
use crate::processing_element::tensors::TensorView;
use crate::processing_element::tensors::dtype::DataType;

impl TaskRunner {
    pub(super) async fn run_program_tile(
        &self,
        task: &PreparedTask,
        plan: GroupPlan<'_>,
        tile: RegionTile,
        admitted: Sender<()>,
    ) -> SimResult {
        let group = self.activity_lanes.create_group(&format!(
            "{} tile {:?}",
            task.id(),
            tile.output_intervals
        ));
        let storage = build_tile_storage(plan, &tile, self.sram_bytes)?;
        let _reservation = self.sram.reserve(storage.arena).await?;
        {
            let mut stats = self.stats.borrow_mut();
            stats.peak_sram_bytes = stats.peak_sram_bytes.max(self.sram.used());
        }
        let _ = admitted.send(());
        self.run_tile_actions(task, plan, tile, group, storage)
            .await
    }

    /// Run an admitted tile group using the same readiness and slot-reuse
    /// dependencies as a standalone tile. The enclosing scope owns admission.
    pub(super) async fn run_tile_actions(
        &self,
        task: &PreparedTask,
        plan: GroupPlan<'_>,
        tile: RegionTile,
        group: Rc<EntityGroup>,
        storage: TileStorage<'_>,
    ) -> SimResult {
        let TileStorage {
            outputs,
            local_allocation,
            ..
        } = storage;
        let mut active = self.start_tile_input_reads(task, plan, &tile, &group);
        let mut progress = TileProgress::new(plan, &local_allocation)?;
        let tile_ref = &tile;
        let group_ref = &group;
        while progress.has_pending_work() || !active.is_empty() {
            for index in progress.take_ready_assignments() {
                active.push(Box::pin(async move {
                    self.run_program_assignment(task, plan, tile_ref, group_ref, index)
                        .await?;
                    Ok(TileCompletion::Assignment(index))
                }));
            }
            for index in progress.take_ready_stores() {
                let name = outputs[index].0;
                let view = &outputs[index].1;
                active.push(Box::pin(async move {
                    self.store_tile_output(view, &format!("{} {name} write", task.id()), group_ref)
                        .await?;
                    Ok(TileCompletion::Store)
                }));
            }
            let Some(result) = active.next().await else {
                return sim_error!("tensor-program assignment dependencies cannot be scheduled");
            };
            progress.record_completion(result?);
        }
        Ok(())
    }

    fn start_tile_input_reads<'a>(
        &'a self,
        task: &'a PreparedTask,
        plan: GroupPlan<'a>,
        tile: &'a RegionTile,
        group: &'a Rc<EntityGroup>,
    ) -> TileActions<'a> {
        let active = TileActions::new();
        for read in collect_initial_input_reads(plan) {
            active.push(Box::pin(async move {
                self.load_input(
                    read.plan_access(tile, plan.region(), &BTreeMap::new())?,
                    &format!("{} {} read", task.id(), read.name),
                    group,
                )
                .await?;
                Ok(TileCompletion::Load(read.name.to_owned()))
            }));
        }
        active
    }

    async fn store_tile_output(
        &self,
        view: &TensorView,
        activity_name: &str,
        group: &Rc<EntityGroup>,
    ) -> SimResult {
        self.access_ranges(
            AccessType::WriteNonPostedRequest,
            view.address_ranges(),
            view.layout().num_access_bytes(),
            &self.activity_lanes.lsu_write,
            activity_name,
            group,
        )
        .await?;
        self.record_write(view)
    }
}

type TileActions<'a> = FuturesUnordered<LocalBoxFuture<'a, Result<TileCompletion, SimError>>>;

struct TileProgress {
    dependencies: BTreeMap<usize, BTreeSet<usize>>,
    input_dependencies: Vec<BTreeSet<String>>,
    pending: Vec<usize>,
    completed: BTreeSet<usize>,
    loaded: BTreeSet<String>,
    pending_stores: BTreeSet<usize>,
    output_producers: Vec<usize>,
}

impl TileProgress {
    fn new(plan: GroupPlan<'_>, allocation: &LocalAllocation) -> Result<Self, SimError> {
        let output_producers = plan
            .outputs()
            .map(|(name, _)| plan.region().producers[name])
            .collect::<Vec<_>>();
        let initial_inputs = collect_initial_input_reads(plan)
            .into_iter()
            .map(|read| read.name.to_owned())
            .collect::<BTreeSet<_>>();
        let input_dependencies = plan
            .region()
            .program
            .assignments
            .iter()
            .map(|assignment| {
                assignment
                    .expr
                    .collect_read_tensor_names()
                    .intersection(&initial_inputs)
                    .cloned()
                    .collect()
            })
            .collect();
        let mut pending = plan.assignments.to_vec();
        // Flat tiles historically issue ready assignments in source order;
        // scoped groups issue in their planned topological group order.
        if matches!(plan.program.schedule, Schedule::Flat(_)) {
            pending.sort_unstable();
        }
        Ok(Self {
            dependencies: build_assignment_dependencies(plan, allocation)?,
            input_dependencies,
            pending,
            completed: BTreeSet::new(),
            loaded: BTreeSet::new(),
            pending_stores: (0..output_producers.len()).collect(),
            output_producers,
        })
    }

    fn has_pending_work(&self) -> bool {
        !self.pending.is_empty() || !self.pending_stores.is_empty()
    }

    fn take_ready_assignments(&mut self) -> Vec<usize> {
        let mut ready = Vec::new();
        self.pending.retain(|index| {
            let available = self.dependencies[index].is_subset(&self.completed)
                && self.input_dependencies[*index].is_subset(&self.loaded);
            if available {
                ready.push(*index);
            }
            !available
        });
        ready
    }

    fn take_ready_stores(&mut self) -> Vec<usize> {
        let ready = self
            .pending_stores
            .iter()
            .copied()
            .filter(|index| self.completed.contains(&self.output_producers[*index]))
            .collect::<Vec<_>>();
        for index in &ready {
            self.pending_stores.remove(index);
        }
        ready
    }

    fn record_completion(&mut self, completion: TileCompletion) {
        match completion {
            TileCompletion::Load(name) => {
                self.loaded.insert(name);
            }
            TileCompletion::Assignment(index) => {
                self.completed.insert(index);
            }
            TileCompletion::Store => {}
        }
    }
}

enum TileCompletion {
    Load(String),
    Assignment(usize),
    Store,
}

pub(super) fn estimate_tile_sram(
    plan: GroupPlan<'_>,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    Ok(build_tile_storage(plan, tile, usize::MAX)?.arena)
}

/// Plan buffer views and SRAM requirements without reserving SRAM.
pub(super) fn build_tile_storage<'a>(
    plan: GroupPlan<'a>,
    tile: &RegionTile,
    available_sram: usize,
) -> Result<TileStorage<'a>, SimError> {
    let region = plan.region();
    let outputs = build_tile_output_views(plan, tile)?;
    let mut arena = tile_input_buffer_bytes(plan, tile, available_sram)?;
    for ((name, _), view) in plan.outputs().zip(&outputs) {
        if plan.program.retained.contains(name) {
            continue;
        }
        add_arena_bytes(&mut arena, align_sram_slot_bytes(view.num_packed_bytes())?)?;
    }
    let local_allocation = allocate_temporaries(plan, tile)?;
    add_arena_bytes(&mut arena, local_allocation.arena_bytes)?;
    for assignment in plan.assignments() {
        add_arena_bytes(
            &mut arena,
            reduction_scratch_bytes(assignment, region, tile)?,
        )?;
    }
    // Scalar expression nodes survive their producer compute batch until the
    // parent operation consumes them. Reserve bounded chunk storage explicitly.
    for assignment in plan.assignments() {
        add_arena_bytes(
            &mut arena,
            scalar_expression_scratch_bytes(assignment, region, tile)?,
        )?;
    }
    if arena > available_sram {
        return sim_error!(
            "{} tile requires {arena} SRAM bytes, available {}",
            "program",
            available_sram
        );
    }
    Ok(TileStorage {
        outputs: plan
            .outputs()
            .map(|(name, _)| name.as_str())
            .zip(outputs)
            .collect(),
        local_allocation,
        arena,
    })
}

fn tile_input_buffer_bytes(
    plan: GroupPlan<'_>,
    tile: &RegionTile,
    available_sram: usize,
) -> Result<usize, SimError> {
    let mut arena = 0usize;
    let region = plan.region();
    for (name, original) in plan.inputs() {
        let Some(indices) = plan
            .assignments()
            .find_map(|assignment| find_read_indices(&assignment.expr, name))
        else {
            continue;
        };
        let bytes = if input_requires_assignment_reads(plan, name) {
            reread_input_buffer_bytes(plan, name, original, tile, region, available_sram)?
        } else {
            mapped_input_buffer_bytes(original, indices, tile, region)?
        };
        add_arena_bytes(&mut arena, bytes)?;
    }
    Ok(arena)
}

fn add_arena_bytes(arena: &mut usize, bytes: usize) -> SimResult {
    *arena = arena
        .checked_add(bytes)
        .ok_or_else(|| SimError("tile SRAM arena overflows".into()))?;
    Ok(())
}

fn reread_input_buffer_bytes(
    plan: GroupPlan<'_>,
    name: &str,
    original: &TensorView,
    tile: &RegionTile,
    region: &ProgramFacts,
    available_sram: usize,
) -> Result<usize, SimError> {
    plan.assignments().try_fold(0usize, |sum, assignment| {
        let bytes = assignment_input_buffer_bytes(
            &assignment.expr,
            name,
            original,
            tile,
            region,
            available_sram,
        )?;
        sum.checked_add(bytes)
            .ok_or_else(|| SimError("tile input SRAM overflows".into()))
    })
}

fn assignment_input_buffer_bytes(
    expr: &Expr,
    name: &str,
    original: &TensorView,
    tile: &RegionTile,
    region: &ProgramFacts,
    available_sram: usize,
) -> Result<usize, SimError> {
    let Some(indices) = find_read_indices(expr, name) else {
        return Ok(0);
    };
    match expr {
        Expr::Reduce { op, over, .. } => {
            if *op != ReductionOp::Sum
                || indices
                    .iter()
                    .any(|index| matches!(index, IndexSpec::Affine { .. }))
            {
                max_window_chunk_buffer_bytes(original, indices, tile, region, over, available_sram)
            } else {
                reduction_input_buffer_bytes(original, indices, tile, over)
            }
        }
        Expr::MatrixMac { over, .. } => reduction_input_buffer_bytes(original, indices, tile, over),
        Expr::Scalar(_) => mapped_input_buffer_bytes(original, indices, tile, region),
    }
}

fn reduction_input_buffer_bytes(
    original: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    over: &[String],
) -> Result<usize, SimError> {
    let mut intervals = tile.output_intervals.clone();
    intervals.extend(tile.reduction_chunks_for(over).next().unwrap_or_default());
    align_sram_slot_bytes(slice_view(original, indices, &intervals)?.num_packed_bytes())
}

fn mapped_input_buffer_bytes(
    original: &TensorView,
    indices: &[IndexSpec],
    tile: &RegionTile,
    region: &ProgramFacts,
) -> Result<usize, SimError> {
    align_sram_slot_bytes(
        if indices
            .iter()
            .any(|index| matches!(index, IndexSpec::Affine { .. }))
        {
            window_chunk_packed_bytes(original, indices, tile, region, &BTreeMap::new())?
        } else {
            slice_view(original, indices, &tile.output_intervals)?.num_packed_bytes()
        },
    )
}

fn reduction_scratch_bytes(
    assignment: &Assignment,
    region: &ProgramFacts,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    match &assignment.expr {
        Expr::Reduce {
            op: ReductionOp::Sum,
            ..
        }
        | Expr::MatrixMac { .. } => sum_reduction_scratch_bytes(assignment, region, tile),
        Expr::Reduce {
            op: ReductionOp::Min | ReductionOp::Max,
            ..
        } => extremum_reduction_scratch_bytes(assignment, region, tile),
        _ => Ok(0),
    }
}

fn sum_reduction_scratch_bytes(
    assignment: &Assignment,
    region: &ProgramFacts,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    let (over, matrix) = match &assignment.expr {
        Expr::Reduce { over, .. } => (over.as_slice(), false),
        Expr::MatrixMac { over, .. } => (over.as_slice(), true),
        Expr::Scalar(_) => unreachable!(),
    };
    let output_dtype = region.value(assignment.primary_result()).unwrap().dtype;
    let separate_cast = matches!(&assignment.expr, Expr::MatrixMac { accumulator, .. } if *accumulator != output_dtype);
    if matrix && tile.reduction_chunk_count_for(over)? == 1 && !separate_cast {
        return Ok(0);
    }
    let input_dtypes = reduction_accumulator_dtypes(&assignment.expr, region);
    let count = count_assignment_output_elements(
        assignment,
        region,
        tile,
        "reduction scratch size overflows",
    )?;
    binding_scratch_bytes(select_reduction_state_binding(
        &input_dtypes,
        output_dtype,
        count,
        true,
    )?)
}

fn reduction_accumulator_dtypes(expr: &Expr, region: &ProgramFacts) -> Vec<DataType> {
    if let Expr::MatrixMac { accumulator, .. } = expr {
        return vec![*accumulator];
    }
    expr.iter_reads()
        .map(|read| region.value(&read.tensor).unwrap().dtype)
        .collect()
}

fn extremum_reduction_scratch_bytes(
    assignment: &Assignment,
    region: &ProgramFacts,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    let Expr::Reduce { over, value, .. } = &assignment.expr else {
        unreachable!();
    };
    if tile.reduction_chunk_count_for(over)? <= 1 {
        return Ok(0);
    }
    let input_dtypes = value
        .iter_reads()
        .map(|expr| region.value(&expr.tensor).unwrap().dtype)
        .collect::<Vec<_>>();
    let count =
        count_assignment_output_elements(assignment, region, tile, "max scratch size overflows")?;
    binding_scratch_bytes(select_reduction_state_binding_for_optional_output(
        &input_dtypes,
        assignment
            .output()
            .map(|name| region.value(name).unwrap().dtype),
        count,
        true,
    )?)
}

fn binding_scratch_bytes(binding: ReductionStateBinding) -> Result<usize, SimError> {
    match binding {
        ReductionStateBinding::Scratch { bytes } => align_sram_slot_bytes(bytes),
        _ => Ok(0),
    }
}

fn scalar_expression_scratch_bytes(
    assignment: &Assignment,
    region: &ProgramFacts,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    let (expr, over, include_root) = match &assignment.expr {
        Expr::Reduce { value, over, .. } => (value, Some(over), true),
        Expr::Scalar(value @ ScalarExpr::Call { .. }) => (value, None, false),
        _ => return Ok(0),
    };
    let calls = count_scalar_call_buffers(expr, include_root);
    let elements = count_scalar_chunk_elements(assignment, region, tile, over.map(Vec::as_slice))?;
    let dtype = assignment.output().map_or_else(
        || find_first_read_dtype(expr, region),
        |name| region.value(name).unwrap().dtype,
    );
    let bytes = align_sram_slot_bytes(
        elements
            .checked_mul(dtype.num_bits())
            .ok_or_else(|| SimError("scalar expression storage overflows".into()))?
            .div_ceil(8),
    )?;
    bytes
        .checked_mul(calls)
        .ok_or_else(|| SimError("scalar expression storage overflows".into()))
}

fn count_scalar_chunk_elements(
    assignment: &Assignment,
    region: &ProgramFacts,
    tile: &RegionTile,
    over: Option<&[String]>,
) -> Result<usize, SimError> {
    let mut elements = count_assignment_output_elements(
        assignment,
        region,
        tile,
        "scalar expression storage overflows",
    )?;
    if let Some(over) = over {
        for range in tile
            .reduction_chunks_for(over)
            .next()
            .unwrap_or_default()
            .values()
        {
            elements = elements
                .checked_mul(range.len())
                .ok_or_else(|| SimError("scalar expression storage overflows".into()))?;
        }
    }
    Ok(elements)
}

fn count_assignment_output_elements(
    assignment: &Assignment,
    region: &ProgramFacts,
    tile: &RegionTile,
    overflow: &str,
) -> Result<usize, SimError> {
    assignment.axes.iter().try_fold(1usize, |count, axis| {
        count
            .checked_mul(
                tile.output_intervals
                    .get(axis)
                    .map_or(region.extent(axis).unwrap(), Range::len),
            )
            .ok_or_else(|| SimError(overflow.into()))
    })
}

pub(super) struct TileStorage<'a> {
    pub outputs: Vec<(&'a str, TensorView)>,
    pub local_allocation: LocalAllocation,
    pub arena: usize,
}

fn count_scalar_call_buffers(expr: &ScalarExpr, include_root: bool) -> usize {
    match expr {
        ScalarExpr::Call { args, .. } => {
            usize::from(include_root)
                + args
                    .iter()
                    .map(|arg| count_scalar_call_buffers(arg, true))
                    .sum::<usize>()
        }
        ScalarExpr::Read(_) => 0,
    }
}
