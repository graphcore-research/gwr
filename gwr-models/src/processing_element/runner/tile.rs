// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeSet;
use std::ops::Range;
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::oneshot;
use futures::future::LocalBoxFuture;
use futures::stream::FuturesUnordered;
use gwr_engine::sim_error;
use gwr_engine::types::{AccessType, SimError, SimResult};

use super::{
    TaskRunner, align8, assignment_dependencies, collect_read_names, find_read,
    max_window_chunk_buffer_bytes, slice_axes, slice_view, window_input_view,
};
use crate::processing_element::program::plan::{LocalAllocation, allocate_temporaries};
use crate::processing_element::program::storage::{
    ReductionStateBinding, reduction_state_binding_for_operands,
    reduction_state_binding_for_optional_output,
};
use crate::processing_element::program::syntax::{ExprSpec, IndexSpec, ReductionOp};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ValidatedRegion;
use crate::processing_element::task::ProgramTaskConfig;
use crate::processing_element::tensors::TensorView;

impl TaskRunner {
    async fn local_payload_phase(
        &self,
        direction: super::super::local_payload::LocalPayloadDirection,
        bytes: usize,
        name: &str,
        group: &Rc<gwr_track::entity::EntityGroup>,
    ) -> SimResult {
        if bytes == 0 {
            return Ok(());
        }
        let service = self
            .local_payload
            .as_ref()
            .ok_or_else(|| SimError("local round requires local service".into()))?;
        // Reservation checks debt/counters before activity. Drop settles
        // canceled logical bytes synchronously; this future owns no
        // byte worker or ACK.
        let reservation = service.reserve(direction, bytes)?;
        let _lane = super::super::activity::ActivityLanes::begin_in_group(
            &self.activity_lanes.local_payload,
            name,
            group,
        );
        reservation.complete().await
    }

    #[expect(clippy::too_many_lines)]
    pub(super) async fn run_program_tile(
        &self,
        config: &ProgramTaskConfig,
        region: &ValidatedRegion,
        tile: RegionTile,
        admitted: oneshot::Sender<()>,
    ) -> SimResult {
        enum TileActionDone {
            Load(String),
            Assignment(usize),
            Store,
        }
        let indexed_ranges = config
            .program
            .indexed_metadata_inputs
            .iter()
            .map(|name| {
                crate::processing_element::program::indexed_metadata::ranges(region, &tile, name)
                    .map(|ranges| (name.clone(), ranges))
            })
            .collect::<Result<std::collections::BTreeMap<_, _>, SimError>>()?;
        let group = self
            .activity_lanes
            .create_group(&format!("{} tile {:?}", config.id, tile.output_intervals));
        let TileStorage {
            inputs,
            outputs,
            local_allocation,
            arena,
            ..
        } = tile_storage(config, region, &tile, self.sram_bytes)?;
        let local_footprints = if config.program.local_round_policy.is_some() {
            let prefix = super::super::local_payload::boundary_arena_bytes(config)?;
            if prefix.checked_add(local_allocation.arena_bytes) != Some(arena) {
                return Err(SimError(
                    "local round source arena differs from actual tile storage".into(),
                ));
            }
            Some(super::super::local_payload::footprints(
                region,
                &tile,
                &local_allocation,
                prefix,
            )?)
        } else {
            None
        };
        let _reservation = self.sram.reserve(arena).await?;
        {
            let mut stats = self.stats.borrow_mut();
            stats.peak_sram_bytes = stats.peak_sram_bytes.max(self.sram.used());
        }
        let _ = admitted.send(());
        let mut active =
            FuturesUnordered::<LocalBoxFuture<'_, Result<TileActionDone, SimError>>>::new();
        let group_ref = &group;
        for (name, view) in config.program.inputs.iter().zip(&inputs) {
            if !config.program.metadata_inputs.contains(name)
                && (is_reduction_input(config, name)
                    || !config
                        .program
                        .assignments
                        .iter()
                        .any(|assignment| find_read(&assignment.expr, name).is_some()))
            {
                continue;
            }
            let indexed = indexed_ranges.get(name).cloned();
            active.push(Box::pin(async move {
                if let Some((ranges, bytes)) = indexed {
                    self.staged_access_ranges(
                        AccessType::ReadRequest,
                        ranges.into_iter(),
                        bytes,
                        &self.activity_lanes.lsu_read,
                        &format!("{} {name} read", config.id),
                        group_ref,
                    )
                    .await?;
                } else {
                    self.staged_access_ranges(
                        AccessType::ReadRequest,
                        view.address_ranges(),
                        view.layout().num_access_bytes(),
                        &self.activity_lanes.lsu_read,
                        &format!("{} {name} read", config.id),
                        group_ref,
                    )
                    .await?;
                }
                Ok(TileActionDone::Load(name.clone()))
            }));
        }
        let dependencies = assignment_dependencies(region, &local_allocation)?;
        let mut pending = region
            .assignment_order
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut completed = BTreeSet::new();
        let mut loaded = BTreeSet::new();
        let mut pending_stores = (0..outputs.len()).collect::<BTreeSet<_>>();
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
        let tile_ref = &tile;
        let local_footprints_ref = &local_footprints;
        while !pending.is_empty() || !pending_stores.is_empty() || !active.is_empty() {
            let ready = pending
                .iter()
                .copied()
                .filter(|index| {
                    if !config
                        .program
                        .metadata_inputs
                        .iter()
                        .all(|name| loaded.contains(name))
                        || !dependencies[index].is_subset(&completed)
                    {
                        return false;
                    }
                    let mut reads = BTreeSet::new();
                    collect_read_names(&config.program.assignments[*index].expr, &mut reads);
                    reads.iter().all(|name| {
                        !config.program.inputs.contains(name)
                            || is_reduction_input(config, name)
                            || loaded.contains(name)
                    })
                })
                .collect::<Vec<_>>();
            for index in ready {
                pending.remove(&index);
                active.push(Box::pin(async move {
                    if let Some(footprints) = local_footprints_ref {
                        self.local_payload_phase(
                            super::super::local_payload::LocalPayloadDirection::Read,
                            footprints[index].read_bytes,
                            &format!("{} assignment{index} local read", config.id),
                            group_ref,
                        )
                        .await?;
                    }
                    self.run_program_assignment(config, region, tile_ref, group_ref, index)
                        .await?;
                    if let Some(footprints) = local_footprints_ref {
                        self.local_payload_phase(
                            super::super::local_payload::LocalPayloadDirection::Write,
                            footprints[index].write_bytes,
                            &format!("{} assignment{index} local write", config.id),
                            group_ref,
                        )
                        .await?;
                    }
                    Ok(TileActionDone::Assignment(index))
                }));
            }
            let ready_stores = pending_stores
                .iter()
                .copied()
                .filter(|index| completed.contains(&output_producers[*index]))
                .collect::<Vec<_>>();
            for index in ready_stores {
                pending_stores.remove(&index);
                let name = &config.program.outputs[index];
                let view = &outputs[index];
                active.push(Box::pin(async move {
                    self.staged_access_ranges(
                        AccessType::WriteNonPostedRequest,
                        view.address_ranges(),
                        view.layout().num_access_bytes(),
                        &self.activity_lanes.lsu_write,
                        &format!("{} {name} write", config.id),
                        group_ref,
                    )
                    .await?;
                    self.record_write(view)?;
                    Ok(TileActionDone::Store)
                }));
            }
            let Some(result) = active.next().await else {
                return sim_error!("tensor-program assignment dependencies cannot be scheduled");
            };
            // The opt-in source path keeps its SRAM reservation until all
            // already-issued child futures settle. Do not publish their
            // readiness or schedule further dependent work after an error.
            let result = if local_footprints.is_some() {
                match result {
                    Ok(done) => done,
                    Err(error) => {
                        while active.next().await.is_some() {}
                        return Err(error);
                    }
                }
            } else {
                result?
            };
            match result {
                TileActionDone::Load(name) => {
                    loaded.insert(name);
                }
                TileActionDone::Assignment(index) => {
                    completed.insert(index);
                }
                TileActionDone::Store => {}
            }
        }
        Ok(())
    }
}

pub(super) fn estimate_tile_sram(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
) -> Result<usize, SimError> {
    Ok(tile_storage(config, region, tile, usize::MAX)?.arena)
}

pub(super) fn tile_storage(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    tile: &RegionTile,
    available_sram: usize,
) -> Result<TileStorage, SimError> {
    let inputs = config
        .program
        .inputs
        .iter()
        .zip(&config.inputs)
        .map(|(name, view)| {
            if config.program.metadata_inputs.contains(name) {
                return Ok(view.clone());
            }
            let Some(indices) = config
                .program
                .assignments
                .iter()
                .find_map(|assignment| find_read(&assignment.expr, name))
            else {
                return Ok(view.clone());
            };
            if is_reduction_input(config, name) {
                Ok(view.clone())
            } else if indices
                .iter()
                .any(|index| matches!(index, IndexSpec::Affine { .. }))
            {
                window_input_view(view, indices, tile, region)
            } else {
                slice_view(view, indices, &tile.output_intervals)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let outputs = config
        .program
        .outputs
        .iter()
        .zip(&config.outputs)
        .map(|(name, view)| {
            let assignment = config
                .program
                .assignments
                .iter()
                .find(|assignment| {
                    &assignment.primary_result() == name
                        || assignment.indices_output.as_ref() == Some(name)
                })
                .ok_or_else(|| SimError(format!("missing assignment for {name}")))?;
            slice_axes(view, &assignment.axes, &tile.output_intervals)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut arena = 0usize;
    for ((name, view), original) in config
        .program
        .inputs
        .iter()
        .zip(&inputs)
        .zip(&config.inputs)
    {
        if !config.program.metadata_inputs.contains(name)
            && !config
                .program
                .assignments
                .iter()
                .any(|assignment| find_read(&assignment.expr, name).is_some())
        {
            continue;
        }
        let bytes = if config.program.metadata_inputs.contains(name) {
            align8(original.num_packed_bytes())?
        } else if is_reduction_input(config, name) {
            config
                .program
                .assignments
                .iter()
                .try_fold(0usize, |sum, assignment| {
                    let action_bytes = match &assignment.expr {
                        ExprSpec::Reduce {
                            op: ReductionOp::Sum,
                            over,
                            value,
                        } => {
                            if let Some(indices) = find_read(value, name) {
                                let mut intervals = tile.output_intervals.clone();
                                intervals.extend(
                                    tile.reduction_chunks_for(over).next().unwrap_or_default(),
                                );
                                if indices
                                    .iter()
                                    .any(|index| matches!(index, IndexSpec::Affine { .. }))
                                {
                                    max_window_chunk_buffer_bytes(
                                        original,
                                        indices,
                                        tile,
                                        region,
                                        over,
                                        available_sram,
                                    )?
                                } else {
                                    align8(
                                        slice_view(original, indices, &intervals)?
                                            .num_packed_bytes(),
                                    )?
                                }
                            } else {
                                0
                            }
                        }
                        ExprSpec::MatrixMac { over, .. } => {
                            if let Some(indices) = find_read(&assignment.expr, name) {
                                let mut intervals = tile.output_intervals.clone();
                                intervals.extend(
                                    tile.reduction_chunks_for(over).next().unwrap_or_default(),
                                );
                                align8(
                                    slice_view(original, indices, &intervals)?.num_packed_bytes(),
                                )?
                            } else {
                                0
                            }
                        }
                        ExprSpec::Reduce {
                            op: ReductionOp::Min | ReductionOp::Max,
                            over,
                            value,
                        } => {
                            if let Some(indices) = find_read(value, name) {
                                max_window_chunk_buffer_bytes(
                                    original,
                                    indices,
                                    tile,
                                    region,
                                    over,
                                    available_sram,
                                )?
                            } else {
                                0
                            }
                        }
                        ExprSpec::Call { .. } | ExprSpec::Read { .. } => {
                            if let Some(indices) = find_read(&assignment.expr, name) {
                                align8(
                                    if indices
                                        .iter()
                                        .any(|index| matches!(index, IndexSpec::Affine { .. }))
                                    {
                                        window_input_view(original, indices, tile, region)?
                                            .num_packed_bytes()
                                    } else {
                                        slice_view(original, indices, &tile.output_intervals)?
                                            .num_packed_bytes()
                                    },
                                )?
                            } else {
                                0
                            }
                        }
                    };
                    sum.checked_add(action_bytes)
                        .ok_or_else(|| SimError("tile input SRAM overflows".into()))
                })?
        } else {
            align8(view.num_packed_bytes())?
        };
        arena = arena
            .checked_add(bytes)
            .ok_or_else(|| SimError("tile SRAM arena overflows".into()))?;
    }
    let input_bytes = arena;
    for view in &outputs {
        arena = arena
            .checked_add(align8(view.num_packed_bytes())?)
            .ok_or_else(|| SimError("tile SRAM arena overflows".into()))?;
    }
    let output_bytes = arena - input_bytes;
    let local_allocation = allocate_temporaries(region, tile)?;
    arena = arena
        .checked_add(local_allocation.arena_bytes)
        .ok_or_else(|| SimError("tile SRAM arena overflows".into()))?;
    {
        for assignment in &config.program.assignments {
            let reduction: Option<(&Vec<String>, Vec<&ExprSpec>, bool)> = match &assignment.expr {
                ExprSpec::Reduce {
                    op: ReductionOp::Sum,
                    over,
                    value,
                } => {
                    let operands = match &**value {
                        ExprSpec::Read { .. } => vec![&**value],
                        ExprSpec::Call { .. } => {
                            let mut names = BTreeSet::new();
                            collect_read_names(value, &mut names);
                            // Only dtype metadata is needed; resolve leaves
                            // below.
                            let mut operands = Vec::new();
                            collect_leaf_reads(value, &mut operands);
                            operands
                        }
                        _ => unreachable!(),
                    };
                    Some((over, operands, false))
                }
                ExprSpec::MatrixMac { args, over, .. } => Some((over, args.iter().collect(), true)),
                _ => None,
            };
            if let Some((over, operands, matrix)) = reduction {
                let names = operands
                    .iter()
                    .map(|operand| {
                        let ExprSpec::Read { tensor, .. } = operand else {
                            unreachable!()
                        };
                        tensor.as_str()
                    })
                    .collect::<Vec<_>>();
                let output_dtype = region.value(&assignment.primary_result()).unwrap().dtype;
                let separate_cast = matches!(&assignment.expr, ExprSpec::MatrixMac { execution: Some(mode), .. } if mode.accumulator != output_dtype);
                if matrix && tile.reduction_chunk_count_for(over)? == 1 && !separate_cast {
                    continue;
                }
                let count = assignment.axes.iter().try_fold(1usize, |count, axis| {
                    count
                        .checked_mul(
                            tile.output_intervals
                                .get(axis)
                                .map_or(region.extent(axis).unwrap(), Range::len),
                        )
                        .ok_or_else(|| SimError("reduction scratch size overflows".into()))
                })?;
                if let ReductionStateBinding::Scratch { bytes } =
                    reduction_state_binding_for_operands(
                        &match &assignment.expr {
                            ExprSpec::MatrixMac {
                                execution: Some(mode),
                                ..
                            } => vec![mode.accumulator],
                            _ => names
                                .iter()
                                .map(|name| region.value(name).unwrap().dtype)
                                .collect::<Vec<_>>(),
                        },
                        region.value(&assignment.primary_result()).unwrap().dtype,
                        count,
                        true,
                    )?
                {
                    arena = arena
                        .checked_add(align8(bytes)?)
                        .ok_or_else(|| SimError("tile SRAM arena overflows".into()))?;
                }
            }
            if let ExprSpec::Reduce {
                op: ReductionOp::Min | ReductionOp::Max,
                over,
                value,
            } = &assignment.expr
            {
                if tile.reduction_chunk_count_for(over)? <= 1 {
                    continue;
                }
                let mut reads = Vec::new();
                collect_leaf_reads(value, &mut reads);
                let input_dtypes = reads
                    .iter()
                    .map(|expr| {
                        let ExprSpec::Read { tensor, .. } = expr else {
                            unreachable!()
                        };
                        region.value(tensor).unwrap().dtype
                    })
                    .collect::<Vec<_>>();
                let count = assignment.axes.iter().try_fold(1usize, |count, axis| {
                    count
                        .checked_mul(
                            tile.output_intervals
                                .get(axis)
                                .map_or(region.extent(axis).unwrap(), Range::len),
                        )
                        .ok_or_else(|| SimError("max scratch size overflows".into()))
                })?;
                if let ReductionStateBinding::Scratch { bytes } =
                    reduction_state_binding_for_optional_output(
                        &input_dtypes,
                        assignment
                            .output
                            .as_ref()
                            .map(|name| region.value(name).unwrap().dtype),
                        count,
                        true,
                    )?
                {
                    arena = arena
                        .checked_add(align8(bytes)?)
                        .ok_or_else(|| SimError("tile SRAM arena overflows".into()))?;
                }
            }
        }
    }
    // Scalar expression nodes survive their producer engine batch until the
    // parent operation consumes them. Reserve bounded chunk storage explicitly.
    for assignment in &config.program.assignments {
        let (expr, over, include_root) = match &assignment.expr {
            ExprSpec::Reduce { value, over, .. } => (&**value, Some(over), true),
            ExprSpec::Call { .. } => (&assignment.expr, None, false),
            _ => continue,
        };
        let calls = scalar_call_buffers(expr, include_root);
        let mut elements = assignment.axes.iter().try_fold(1usize, |count, axis| {
            count
                .checked_mul(
                    tile.output_intervals
                        .get(axis)
                        .map_or(region.extent(axis).unwrap(), Range::len),
                )
                .ok_or_else(|| SimError("scalar expression storage overflows".into()))
        })?;
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
        let bits = region
            .value(&assignment.primary_result())
            .unwrap()
            .dtype
            .num_bits();
        let bytes = align8(
            elements
                .checked_mul(bits)
                .ok_or_else(|| SimError("scalar expression storage overflows".into()))?
                .div_ceil(8),
        )?;
        arena = arena
            .checked_add(
                bytes
                    .checked_mul(calls)
                    .ok_or_else(|| SimError("scalar expression storage overflows".into()))?,
            )
            .ok_or_else(|| SimError("tile SRAM arena overflows".into()))?;
    }
    if arena > available_sram {
        return sim_error!(
            "{} tile requires {arena} SRAM bytes, available {}",
            config.id,
            available_sram
        );
    }
    Ok(TileStorage {
        inputs,
        outputs,
        local_allocation,
        arena,
        input_bytes,
        output_bytes,
    })
}

pub(super) struct TileStorage {
    pub inputs: Vec<TensorView>,
    pub outputs: Vec<TensorView>,
    pub local_allocation: LocalAllocation,
    pub arena: usize,
    pub input_bytes: usize,
    pub output_bytes: usize,
}

pub(super) fn is_reduction_input(config: &ProgramTaskConfig, name: &str) -> bool {
    let mappings = config
        .program
        .assignments
        .iter()
        .filter_map(|assignment| find_read(&assignment.expr, name))
        .collect::<Vec<_>>();
    if mappings.windows(2).any(|pair| pair[0] != pair[1]) {
        return true;
    }
    config
        .program
        .assignments
        .iter()
        .any(|assignment| match &assignment.expr {
            ExprSpec::Reduce { value, .. } => find_read(value, name).is_some(),
            ExprSpec::MatrixMac { over, .. } => {
                !over.is_empty() && find_read(&assignment.expr, name).is_some()
            }
            _ => false,
        })
}

fn collect_leaf_reads<'a>(expr: &'a ExprSpec, reads: &mut Vec<&'a ExprSpec>) {
    match expr {
        ExprSpec::Read { .. } => reads.push(expr),
        ExprSpec::Call { args, .. } => {
            for arg in args {
                collect_leaf_reads(arg, reads);
            }
        }
        _ => {}
    }
}

fn scalar_call_buffers(expr: &ExprSpec, include_root: bool) -> usize {
    match expr {
        ExprSpec::Call { args, .. } => {
            usize::from(include_root)
                + args
                    .iter()
                    .map(|arg| scalar_call_buffers(arg, true))
                    .sum::<usize>()
        }
        _ => 0,
    }
}
