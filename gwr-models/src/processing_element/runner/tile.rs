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
use crate::processing_element::program::plan::{LocalAllocation, allocate_temporaries};
use crate::processing_element::program::storage::{
    ReductionStateBinding, align_sram_slot_bytes, select_reduction_state_binding,
    select_reduction_state_binding_for_optional_output,
};
use crate::processing_element::program::syntax::{
    AssignmentSpec, ExprSpec, IndexSpec, ProgramSpec, ReductionOp,
};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::{RegionBindings, ValidatedRegion};
use crate::processing_element::task::ProgramTaskConfig;
use crate::processing_element::tensors::TensorView;
use crate::processing_element::tensors::dtype::DataType;

impl TaskRunner {
    pub(super) async fn run_program_tile(
        &self,
        config: &ProgramTaskConfig,
        region: &ValidatedRegion,
        tile: RegionTile,
        admitted: Sender<()>,
    ) -> SimResult {
        let group = self
            .activity_lanes
            .create_group(&format!("{} tile {:?}", config.id, tile.output_intervals));
        let storage = build_tile_storage(config, region, &tile, self.sram_bytes)?;
        let _reservation = self.sram.reserve(storage.arena).await?;
        {
            let mut stats = self.stats.borrow_mut();
            stats.peak_sram_bytes = stats.peak_sram_bytes.max(self.sram.used());
        }
        let _ = admitted.send(());
        self.run_tile_actions(config, region, tile, group, storage)
            .await
    }

    /// Run an admitted tile group using the same readiness and slot-reuse
    /// dependencies as a standalone tile. The enclosing scope owns admission.
    pub(super) async fn run_tile_actions(
        &self,
        config: &ProgramTaskConfig,
        region: &ValidatedRegion,
        tile: RegionTile,
        group: Rc<EntityGroup>,
        storage: TileStorage,
    ) -> SimResult {
        let TileStorage {
            outputs,
            local_allocation,
            ..
        } = storage;
        let mut active = self.start_tile_input_reads(config, region, &tile, &group);
        let mut progress = TileProgress::new(config, region, &local_allocation)?;
        let tile_ref = &tile;
        let group_ref = &group;
        while progress.has_pending_work() || !active.is_empty() {
            for index in progress.take_ready_assignments() {
                active.push(Box::pin(async move {
                    self.run_program_assignment(config, region, tile_ref, group_ref, index)
                        .await?;
                    Ok(TileCompletion::Assignment(index))
                }));
            }
            for index in progress.take_ready_stores() {
                let name = &config.program.outputs[index];
                let view = &outputs[index];
                active.push(Box::pin(async move {
                    self.store_tile_output(view, &format!("{} {name} write", config.id), group_ref)
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
        config: &'a ProgramTaskConfig,
        region: &'a ValidatedRegion,
        tile: &'a RegionTile,
        group: &'a Rc<EntityGroup>,
    ) -> TileActions<'a> {
        let active = TileActions::new();
        for read in collect_initial_input_reads(config) {
            active.push(Box::pin(async move {
                self.load_input(
                    read.plan_access(tile, region, &BTreeMap::new())?,
                    &format!("{} {} read", config.id, read.name),
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
    pending: BTreeSet<usize>,
    completed: BTreeSet<usize>,
    loaded: BTreeSet<String>,
    pending_stores: BTreeSet<usize>,
    output_producers: Vec<usize>,
}

impl TileProgress {
    fn new(
        config: &ProgramTaskConfig,
        region: &ValidatedRegion,
        allocation: &LocalAllocation,
    ) -> Result<Self, SimError> {
        let output_producers = config
            .program
            .outputs
            .iter()
            .map(|name| {
                config
                    .program
                    .assignments
                    .iter()
                    .position(|assignment| {
                        &assignment.primary_result() == name
                            || assignment.indices_output.as_ref() == Some(name)
                    })
                    .ok_or_else(|| SimError(format!("missing assignment for {name}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let initial_inputs = collect_initial_input_reads(config)
            .into_iter()
            .map(|read| read.name.to_owned())
            .collect::<BTreeSet<_>>();
        let input_dependencies = config
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
        Ok(Self {
            dependencies: build_assignment_dependencies(region, allocation)?,
            input_dependencies,
            pending: region.assignment_order.iter().copied().collect(),
            completed: BTreeSet::new(),
            loaded: BTreeSet::new(),
            pending_stores: (0..config.outputs.len()).collect(),
            output_producers,
        })
    }

    fn has_pending_work(&self) -> bool {
        !self.pending.is_empty() || !self.pending_stores.is_empty()
    }

    fn take_ready_assignments(&mut self) -> Vec<usize> {
        let ready = self
            .pending
            .iter()
            .copied()
            .filter(|index| {
                self.dependencies[index].is_subset(&self.completed)
                    && self.input_dependencies[*index].is_subset(&self.loaded)
            })
            .collect::<Vec<_>>();
        for index in &ready {
            self.pending.remove(index);
        }
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
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    Ok(build_tile_storage(config, region, tile, usize::MAX)?.arena)
}

/// Build the configuration and validated region for one assignment group.
/// Enclosing-scope values are already available; only group-local temporaries
/// participate in the ordinary slot allocator.
pub(super) fn build_group_config(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    indices: &[usize],
    retained: &BTreeSet<String>,
) -> (ProgramTaskConfig, ValidatedRegion) {
    let mut program = ProgramSpec {
        inputs: config.program.inputs.clone(),
        outputs: Vec::new(),
        temporaries: config
            .program
            .temporaries
            .iter()
            .filter(|(name, _)| !retained.contains(*name))
            .map(|(name, temporary)| (name.clone(), temporary.clone()))
            .collect(),
        assignments: indices
            .iter()
            .map(|&index| config.program.assignments[index].clone())
            .collect(),
        extents: config.program.extents.clone(),
    };
    let mut outputs = Vec::new();
    for (name, view) in config.program.outputs.iter().zip(&config.outputs) {
        if program.assignments.iter().any(|assignment| {
            assignment.output.as_ref() == Some(name)
                || assignment.indices_output.as_ref() == Some(name)
        }) {
            program.outputs.push(name.clone());
            outputs.push(view.clone());
        }
    }
    let phase = ProgramTaskConfig {
        id: config.id.clone(),
        program: program.clone(),
        tiling: config.tiling.clone(),
        inputs: config.inputs.clone(),
        outputs: outputs.clone(),
    };
    let phase_region = ValidatedRegion {
        program,
        bindings: RegionBindings {
            inputs: region.bindings.inputs.clone(),
            outputs,
        },
        values: region.values.clone(),
        extents: region.extents.clone(),
        assignment_order: (0..indices.len()).collect(),
    };
    (phase, phase_region)
}

pub(super) fn build_scoped_group_storage(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
    retained: &BTreeSet<String>,
) -> Result<TileStorage, SimError> {
    let mut storage = build_tile_storage(config, region, tile, usize::MAX)?;
    // A cross-phase boundary result is stored directly from its retained local
    // allocation. Group-local boundary buffers are still reserved normally.
    for (name, view) in config.program.outputs.iter().zip(&storage.outputs) {
        if retained.contains(name) {
            storage.arena = storage
                .arena
                .checked_sub(align_sram_slot_bytes(view.num_packed_bytes())?)
                .ok_or_else(|| SimError("retained output storage is inconsistent".into()))?;
        }
    }
    Ok(storage)
}

/// Plan buffer views and SRAM requirements without reserving SRAM.
pub(super) fn build_tile_storage(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
    available_sram: usize,
) -> Result<TileStorage, SimError> {
    let outputs = build_tile_output_views(config, tile)?;
    let mut arena = tile_input_buffer_bytes(config, region, tile, available_sram)?;
    for view in &outputs {
        add_arena_bytes(&mut arena, align_sram_slot_bytes(view.num_packed_bytes())?)?;
    }
    let local_allocation = allocate_temporaries(region, tile)?;
    add_arena_bytes(&mut arena, local_allocation.arena_bytes)?;
    for assignment in &config.program.assignments {
        add_arena_bytes(
            &mut arena,
            reduction_scratch_bytes(assignment, region, tile)?,
        )?;
    }
    // Scalar expression nodes survive their producer compute batch until the
    // parent operation consumes them. Reserve bounded chunk storage explicitly.
    for assignment in &config.program.assignments {
        add_arena_bytes(
            &mut arena,
            scalar_expression_scratch_bytes(assignment, region, tile)?,
        )?;
    }
    if arena > available_sram {
        return sim_error!(
            "{} tile requires {arena} SRAM bytes, available {}",
            config.id,
            available_sram
        );
    }
    Ok(TileStorage {
        outputs,
        local_allocation,
        arena,
    })
}

fn tile_input_buffer_bytes(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
    available_sram: usize,
) -> Result<usize, SimError> {
    let mut arena = 0usize;
    for (name, original) in config.program.inputs.iter().zip(&config.inputs) {
        let Some(indices) = config
            .program
            .assignments
            .iter()
            .find_map(|assignment| find_read_indices(&assignment.expr, name))
        else {
            continue;
        };
        let bytes = if input_requires_assignment_reads(config, name) {
            reread_input_buffer_bytes(config, name, original, tile, region, available_sram)?
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
    config: &ProgramTaskConfig,
    name: &str,
    original: &TensorView,
    tile: &RegionTile,
    region: &ValidatedRegion,
    available_sram: usize,
) -> Result<usize, SimError> {
    config
        .program
        .assignments
        .iter()
        .try_fold(0usize, |sum, assignment| {
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
    expr: &ExprSpec,
    name: &str,
    original: &TensorView,
    tile: &RegionTile,
    region: &ValidatedRegion,
    available_sram: usize,
) -> Result<usize, SimError> {
    let Some(indices) = find_read_indices(expr, name) else {
        return Ok(0);
    };
    match expr {
        ExprSpec::Reduce { op, over, .. } => {
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
        ExprSpec::MatrixMac { over, .. } => {
            reduction_input_buffer_bytes(original, indices, tile, over)
        }
        ExprSpec::Call { .. } | ExprSpec::Read { .. } => {
            mapped_input_buffer_bytes(original, indices, tile, region)
        }
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
    region: &ValidatedRegion,
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
    assignment: &AssignmentSpec,
    region: &ValidatedRegion,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    match &assignment.expr {
        ExprSpec::Reduce {
            op: ReductionOp::Sum,
            ..
        }
        | ExprSpec::MatrixMac { .. } => sum_reduction_scratch_bytes(assignment, region, tile),
        ExprSpec::Reduce {
            op: ReductionOp::Min | ReductionOp::Max,
            ..
        } => extremum_reduction_scratch_bytes(assignment, region, tile),
        _ => Ok(0),
    }
}

fn sum_reduction_scratch_bytes(
    assignment: &AssignmentSpec,
    region: &ValidatedRegion,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    let (over, matrix) = match &assignment.expr {
        ExprSpec::Reduce { over, .. } => (over, false),
        ExprSpec::MatrixMac { over, .. } => (over, true),
        _ => unreachable!(),
    };
    let output_dtype = region.value(&assignment.primary_result()).unwrap().dtype;
    let separate_cast = matches!(&assignment.expr, ExprSpec::MatrixMac { accumulator, .. } if *accumulator != output_dtype);
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

fn reduction_accumulator_dtypes(expr: &ExprSpec, region: &ValidatedRegion) -> Vec<DataType> {
    let value = match expr {
        ExprSpec::MatrixMac { accumulator, .. } => return vec![*accumulator],
        ExprSpec::Reduce { value, .. }
            if matches!(&**value, ExprSpec::Read { .. } | ExprSpec::Call { .. }) =>
        {
            value
        }
        _ => unreachable!(),
    };
    value
        .iter_reads()
        .map(|read| {
            let ExprSpec::Read { tensor, .. } = read else {
                unreachable!()
            };
            region.value(tensor).unwrap().dtype
        })
        .collect()
}

fn extremum_reduction_scratch_bytes(
    assignment: &AssignmentSpec,
    region: &ValidatedRegion,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    let ExprSpec::Reduce { over, value, .. } = &assignment.expr else {
        unreachable!();
    };
    if tile.reduction_chunk_count_for(over)? <= 1 {
        return Ok(0);
    }
    let input_dtypes = value
        .iter_reads()
        .map(|expr| {
            let ExprSpec::Read { tensor, .. } = expr else {
                unreachable!();
            };
            region.value(tensor).unwrap().dtype
        })
        .collect::<Vec<_>>();
    let count =
        count_assignment_output_elements(assignment, region, tile, "max scratch size overflows")?;
    binding_scratch_bytes(select_reduction_state_binding_for_optional_output(
        &input_dtypes,
        assignment
            .output
            .as_ref()
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
    assignment: &AssignmentSpec,
    region: &ValidatedRegion,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    let (expr, over, include_root) = match &assignment.expr {
        ExprSpec::Reduce { value, over, .. } => (&**value, Some(over), true),
        ExprSpec::Call { .. } => (&assignment.expr, None, false),
        _ => return Ok(0),
    };
    let calls = count_scalar_call_buffers(expr, include_root);
    let elements = count_scalar_chunk_elements(assignment, region, tile, over.map(Vec::as_slice))?;
    let dtype = assignment.output.as_ref().map_or_else(
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
    assignment: &AssignmentSpec,
    region: &ValidatedRegion,
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
    assignment: &AssignmentSpec,
    region: &ValidatedRegion,
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

pub(super) struct TileStorage {
    pub outputs: Vec<TensorView>,
    pub local_allocation: LocalAllocation,
    pub arena: usize,
}

fn count_scalar_call_buffers(expr: &ExprSpec, include_root: bool) -> usize {
    match expr {
        ExprSpec::Call { args, .. } => {
            usize::from(include_root)
                + args
                    .iter()
                    .map(|arg| count_scalar_call_buffers(arg, true))
                    .sum::<usize>()
        }
        _ => 0,
    }
}
