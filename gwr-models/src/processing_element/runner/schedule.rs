// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Execution of inferred scopes and dependency-driven tile groups. Locals have
//! no boundary addresses; their live storage is included in the SRAM working
//! set.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use futures::future::LocalBoxFuture;
use gwr_engine::types::{SimError, SimResult};

use super::tile::{build_group_config, build_scoped_group_storage};
use super::{TaskRunner, validate_assignment_compute_support};
use crate::processing_element::compute::ComputeModel;
use crate::processing_element::program::schedule::{ScopedSchedule, ScopedStep};
use crate::processing_element::program::storage::align_sram_slot_bytes;
use crate::processing_element::program::syntax::{ExprSpec, TileSpec};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ValidatedRegion;
use crate::processing_element::task::ProgramTaskConfig;

impl TaskRunner {
    pub(super) async fn run_scoped_program(
        &self,
        config: &ProgramTaskConfig,
        region: &ValidatedRegion,
        schedule: &ScopedSchedule,
    ) -> SimResult {
        let peak = estimate_scoped_peak_sram(config, region, schedule, Some(&self.compute_model))?;
        if peak > self.sram_bytes {
            return Err(SimError(format!(
                "{} schedule requires {peak} SRAM bytes, available {}",
                config.id, self.sram_bytes
            )));
        }
        // Reserve the maximum live set once: phases reuse this arena, avoiding
        // nested admission waits while outer-scope statistics remain live.
        let _reservation = self.sram.reserve(peak).await?;
        {
            let mut stats = self.stats.borrow_mut();
            stats.peak_sram_bytes = stats.peak_sram_bytes.max(self.sram.used());
        }
        let retained = collect_retained_values(schedule);
        self.run_scope(config, region, schedule, &Intervals::new(), &retained)
            .await
    }

    fn run_scope<'a>(
        &'a self,
        config: &'a ProgramTaskConfig,
        region: &'a ValidatedRegion,
        scope: &'a ScopedSchedule,
        outer: &'a Intervals,
        retained: &'a BTreeSet<String>,
    ) -> LocalBoxFuture<'a, SimResult> {
        Box::pin(async move {
            for intervals in ScopeIntervals::new(scope, outer, region) {
                for step in &scope.steps {
                    match step {
                        ScopedStep::Tiles(child) => {
                            self.run_scope(config, region, child, &intervals, retained)
                                .await?;
                        }
                        ScopedStep::Group(indices) => {
                            self.run_scoped_group(config, region, indices, &intervals, retained)
                                .await?;
                        }
                    }
                }
            }
            Ok(())
        })
    }

    async fn run_scoped_group(
        &self,
        config: &ProgramTaskConfig,
        region: &ValidatedRegion,
        indices: &[usize],
        intervals: &Intervals,
        retained: &BTreeSet<String>,
    ) -> SimResult {
        let tile = build_group_tile(indices, intervals, region, config.tiling.as_ref())?;
        let (phase, phase_region) = build_group_config(config, region, indices, retained);
        let storage = build_scoped_group_storage(&phase, &phase_region, &tile, retained)?;
        let group = self
            .activity_lanes
            .create_group(&format!("{} tile {:?}", config.id, tile.output_intervals));
        self.run_tile_actions(&phase, &phase_region, tile, group, storage)
            .await
    }
}

pub(in crate::processing_element) fn estimate_scoped_peak_sram(
    config: &ProgramTaskConfig,
    region: &ValidatedRegion,
    schedule: &ScopedSchedule,
    compute_model: Option<&ComputeModel>,
) -> Result<usize, SimError> {
    let mut peak = 0;
    let retained = collect_retained_values(schedule);
    visit_groups(
        schedule,
        region,
        config.tiling.as_ref(),
        &mut |indices, tile, local| {
            if let Some(model) = compute_model {
                for &index in indices {
                    validate_assignment_compute_support(model, region, index, tile)?;
                }
            }
            let (phase, phase_region) = build_group_config(config, region, indices, &retained);
            let storage = build_scoped_group_storage(&phase, &phase_region, tile, &retained)?;
            peak = peak.max(
                local
                    .checked_add(storage.arena)
                    .ok_or_else(|| SimError("schedule working set overflows".into()))?,
            );
            Ok(())
        },
    )?;
    Ok(peak)
}

pub(super) fn collect_retained_values(scope: &ScopedSchedule) -> BTreeSet<String> {
    let mut result = scope.local_values.iter().cloned().collect::<BTreeSet<_>>();
    for step in &scope.steps {
        if let ScopedStep::Tiles(child) = step {
            result.extend(collect_retained_values(child));
        }
    }
    result
}

/// Traverse the same assignment invocations used by execution. Traffic
/// estimation and compute validation use this traversal, including every
/// clipped tail and row scope.
pub(super) fn visit_groups(
    scope: &ScopedSchedule,
    region: &ValidatedRegion,
    tiling: Option<&TileSpec>,
    visit: &mut impl FnMut(&[usize], &RegionTile, usize) -> SimResult,
) -> SimResult {
    fn walk(
        scope: &ScopedSchedule,
        region: &ValidatedRegion,
        outer: &Intervals,
        inherited: usize,
        tiling: Option<&TileSpec>,
        visit: &mut impl FnMut(&[usize], &RegionTile, usize) -> SimResult,
    ) -> SimResult {
        for intervals in ScopeIntervals::new(scope, outer, region) {
            let live = calculate_live_local_bytes_by_step(scope, &intervals, region)?;
            for (step, local) in scope.steps.iter().zip(live) {
                let bytes = inherited
                    .checked_add(local)
                    .ok_or_else(|| SimError("schedule working set overflows".into()))?;
                match step {
                    ScopedStep::Group(indices) => visit(
                        indices,
                        &build_group_tile(indices, &intervals, region, tiling)?,
                        bytes,
                    )?,
                    ScopedStep::Tiles(child) => {
                        walk(child, region, &intervals, bytes, tiling, visit)?;
                    }
                }
            }
        }
        Ok(())
    }
    walk(scope, region, &Intervals::new(), 0, tiling, visit)
}

/// A lazy Cartesian product of this scope's loops, preserving enclosing ranges.
struct ScopeIntervals {
    outer: Intervals,
    axes: Vec<(String, usize, usize)>,
    starts: Vec<usize>,
    done: bool,
}

impl ScopeIntervals {
    fn new(scope: &ScopedSchedule, outer: &Intervals, region: &ValidatedRegion) -> Self {
        let axes = scope
            .tiles
            .iter()
            .map(|(axis, step)| (axis.clone(), region.extents[axis], *step))
            .collect::<Vec<_>>();
        Self {
            outer: outer.clone(),
            starts: vec![0; axes.len()],
            axes,
            done: false,
        }
    }
}

impl Iterator for ScopeIntervals {
    type Item = Intervals;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut result = self.outer.clone();
        for ((axis, extent, step), start) in self.axes.iter().zip(&self.starts) {
            result.insert(
                axis.clone(),
                *start..(*extent).min(start.saturating_add(*step)),
            );
        }
        self.done = true;
        for index in (0..self.axes.len()).rev() {
            let (_, extent, step) = &self.axes[index];
            if let Some(next) = self.starts[index].checked_add(*step)
                && next < *extent
            {
                self.starts[index] = next;
                self.done = false;
                break;
            }
            self.starts[index] = 0;
        }
        Some(result)
    }
}

fn build_group_tile(
    indices: &[usize],
    intervals: &Intervals,
    region: &ValidatedRegion,
    tiling: Option<&TileSpec>,
) -> Result<RegionTile, SimError> {
    let mut reductions = BTreeMap::new();
    for &index in indices {
        if let ExprSpec::Reduce { over, .. } | ExprSpec::MatrixMac { over, .. } =
            &region.program.assignments[index].expr
        {
            for axis in over {
                let step = tiling
                    .and_then(|spec| spec.sizes.get(axis))
                    .copied()
                    .unwrap_or(region.extents[axis]);
                reductions.insert(axis.clone(), step);
            }
        }
    }
    RegionTile::new(intervals.clone(), reductions, region)
}

/// Live local bytes at each step. Parent scopes keep a tensor across completed
/// child loops; a temporary used only inside a child is owned by that child.
fn calculate_live_local_bytes_by_step(
    scope: &ScopedSchedule,
    intervals: &Intervals,
    region: &ValidatedRegion,
) -> Result<Vec<usize>, SimError> {
    let mut live = vec![0usize; scope.steps.len()];
    for name in &scope.local_values {
        let bytes = local_value_bytes(name, intervals, region)?;
        let first = scope
            .steps
            .iter()
            .position(|step| step_uses(step, name, region))
            .unwrap();
        let last = scope
            .steps
            .iter()
            .rposition(|step| step_uses(step, name, region))
            .unwrap();
        for total in &mut live[first..=last] {
            *total = total
                .checked_add(bytes)
                .ok_or_else(|| SimError("schedule local storage overflows".into()))?;
        }
    }
    Ok(live)
}

fn step_uses(step: &ScopedStep, name: &str, region: &ValidatedRegion) -> bool {
    match step {
        ScopedStep::Group(indices) => indices.iter().any(|&index| {
            let assignment = &region.program.assignments[index];
            assignment.output.as_deref() == Some(name)
                || assignment.indices_output.as_deref() == Some(name)
                || assignment
                    .expr
                    .iter_read_tensor_names()
                    .any(|read| read == name)
        }),
        ScopedStep::Tiles(child) => child.steps.iter().any(|step| step_uses(step, name, region)),
    }
}

fn local_value_bytes(
    name: &str,
    intervals: &Intervals,
    region: &ValidatedRegion,
) -> Result<usize, SimError> {
    let producer = region
        .program
        .assignments
        .iter()
        .find(|assignment| {
            assignment.output.as_deref() == Some(name)
                || assignment.indices_output.as_deref() == Some(name)
        })
        .unwrap();
    let value = &region.values[name];
    let elements =
        producer
            .axes
            .iter()
            .zip(&value.shape)
            .try_fold(1usize, |count, (axis, dim)| {
                let len = if *dim == 1 {
                    1
                } else {
                    intervals.get(axis).map_or(*dim, Range::len)
                };
                count
                    .checked_mul(len)
                    .ok_or_else(|| SimError("schedule local shape overflows".into()))
            })?;
    align_sram_slot_bytes(
        elements
            .checked_mul(value.dtype.num_bits())
            .ok_or_else(|| SimError("schedule local storage overflows".into()))?
            .div_ceil(8),
    )
}

type Intervals = BTreeMap<String, Range<usize>>;
