// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Lazy iteration of the tile intervals explicitly requested by a compute node.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;

use super::syntax::{ExprSpec, TileSpec};
use super::validate::ValidatedRegion;

pub fn plan_tiles(
    region: &ValidatedRegion,
    tiling: Option<&TileSpec>,
) -> Result<TileSchedule, SimError> {
    let mut parallel = Vec::new();
    let mut seen_parallel = BTreeSet::new();
    let mut reduction = BTreeSet::new();
    for assignment in &region.program.assignments {
        if assignment
            .output
            .as_ref()
            .is_some_and(|name| region.program.outputs.contains(name))
            || assignment
                .indices_output
                .as_ref()
                .is_some_and(|name| region.program.outputs.contains(name))
        {
            for axis in &assignment.axes {
                if seen_parallel.insert(axis.clone()) {
                    parallel.push(axis.clone());
                }
            }
        }
        collect_reductions(&assignment.expr, &mut reduction);
    }
    let mut known = seen_parallel;
    known.extend(reduction.iter().cloned());
    let default_sizes = BTreeMap::new();
    let sizes = tiling.map_or(&default_sizes, |spec| &spec.sizes);
    for (axis, step) in sizes {
        if !known.contains(axis) {
            return sim_error!("unknown tile axis {axis}");
        }
        if *step == 0 {
            return sim_error!("tile size for {axis} must be positive");
        }
        if *step < region.extents[axis] && parallel.contains(axis) && reduction.contains(axis) {
            for assignment in &region.program.assignments {
                if !assignment.axes.contains(axis)
                    && assignment
                        .output
                        .as_ref()
                        .is_some_and(|name| region.program.outputs.contains(name))
                {
                    return sim_error!(
                        "recomputed reduction values must be local temporaries when axis {axis} is tiled"
                    );
                }
                if reduction_reads_materialized_axis(&assignment.expr, axis, region) {
                    return sim_error!(
                        "reduction of tiled local axis {axis} requires an inline scalar expression or a materialized full-row plan"
                    );
                }
            }
        }
        if *step < region.extents[axis]
            && parallel.contains(axis)
            && !reduction.contains(axis)
            && region
                .program
                .assignments
                .iter()
                .any(|assignment| !assignment.axes.contains(axis))
        {
            return sim_error!("tile axis {axis} must occur in every assignment output");
        }
    }
    let axis_steps = |names: Vec<String>| -> Result<Vec<AxisStep>, SimError> {
        names
            .into_iter()
            .map(|name| {
                let extent = *region
                    .extents
                    .get(&name)
                    .ok_or_else(|| SimError(format!("unconstrained tile axis {name}")))?;
                let step = sizes.get(&name).copied().unwrap_or(extent);
                Ok(AxisStep { name, extent, step })
            })
            .collect()
    };
    Ok(TileSchedule {
        outputs: axis_steps(parallel)?,
        reductions: axis_steps(reduction.into_iter().collect())?,
    })
}

#[derive(Clone, Debug)]
pub struct TileSchedule {
    outputs: Vec<AxisStep>,
    reductions: Vec<AxisStep>,
}

impl TileSchedule {
    /// Consume a schedule into an owned, lazy output-tile iterator.
    pub fn into_tiles(self) -> impl Iterator<Item = RegionTile> {
        IntervalProduct::new(self.outputs).map(move |output_intervals| RegionTile {
            output_intervals,
            reductions: self.reductions.clone(),
        })
    }

    pub fn tiles(&self) -> impl Iterator<Item = RegionTile> + '_ {
        IntervalProduct::new(self.outputs.clone()).map(|output_intervals| RegionTile {
            output_intervals,
            reductions: self.reductions.clone(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct RegionTile {
    pub output_intervals: BTreeMap<String, Range<usize>>,
    reductions: Vec<AxisStep>,
}

impl RegionTile {
    pub fn reduction_chunks(&self) -> impl Iterator<Item = BTreeMap<String, Range<usize>>> + '_ {
        IntervalProduct::new(self.reductions.clone())
    }

    pub fn reduction_chunk_count(&self) -> Result<usize, SimError> {
        self.reductions.iter().try_fold(1usize, |count, axis| {
            count
                .checked_mul(axis.extent.div_ceil(axis.step))
                .ok_or_else(|| SimError("reduction chunk count overflows".into()))
        })
    }

    pub fn reduction_chunks_for(
        &self,
        axes: &[String],
    ) -> impl Iterator<Item = BTreeMap<String, Range<usize>>> + '_ {
        let selected = self
            .reductions
            .iter()
            .filter(|step| axes.contains(&step.name))
            .cloned()
            .collect::<Vec<_>>();
        IntervalProduct::new(selected)
    }

    pub fn reduction_chunk_count_for(&self, axes: &[String]) -> Result<usize, SimError> {
        self.reductions
            .iter()
            .filter(|step| axes.contains(&step.name))
            .try_fold(1usize, |count, step| {
                count
                    .checked_mul(step.extent.div_ceil(step.step))
                    .ok_or_else(|| SimError("reduction chunk count overflows".into()))
            })
    }
}

struct IntervalProduct {
    axes: Vec<AxisStep>,
    starts: Vec<usize>,
    exhausted: bool,
}

impl IntervalProduct {
    fn new(axes: Vec<AxisStep>) -> Self {
        let starts = vec![0; axes.len()];
        Self {
            axes,
            starts,
            exhausted: false,
        }
    }
}

impl Iterator for IntervalProduct {
    type Item = BTreeMap<String, Range<usize>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        let result = self
            .axes
            .iter()
            .zip(&self.starts)
            .map(|(axis, start)| {
                (
                    axis.name.clone(),
                    *start..axis.extent.min(start.saturating_add(axis.step)),
                )
            })
            .collect();
        self.exhausted = true;
        for index in (0..self.axes.len()).rev() {
            if let Some(next) = self.starts[index].checked_add(self.axes[index].step)
                && next < self.axes[index].extent
            {
                self.starts[index] = next;
                for after in index + 1..self.axes.len() {
                    self.starts[after] = 0;
                }
                self.exhausted = false;
                break;
            }
        }
        Some(result)
    }
}

#[derive(Clone, Debug)]
struct AxisStep {
    name: String,
    extent: usize,
    step: usize,
}

fn collect_reductions(expr: &ExprSpec, axes: &mut BTreeSet<String>) {
    match expr {
        ExprSpec::Read { .. } => {}
        ExprSpec::Call { args, .. } => {
            for arg in args {
                collect_reductions(arg, axes);
            }
        }
        ExprSpec::MatrixMac { args, over, .. } => {
            axes.extend(over.iter().cloned());
            for arg in args {
                collect_reductions(arg, axes);
            }
        }
        ExprSpec::Reduce { over, value, .. } => {
            axes.extend(over.iter().cloned());
            collect_reductions(value, axes);
        }
    }
}

fn reduction_reads_materialized_axis(
    expr: &ExprSpec,
    axis: &str,
    region: &ValidatedRegion,
) -> bool {
    match expr {
        ExprSpec::Reduce { over, value, .. } if over.iter().any(|name| name == axis) => {
            reads_local_axis(value, axis, region)
        }
        ExprSpec::MatrixMac { over, args, .. } if over.iter().any(|name| name == axis) => {
            args.iter().any(|arg| reads_local_axis(arg, axis, region))
        }
        ExprSpec::Call { args, .. } => args
            .iter()
            .any(|arg| reduction_reads_materialized_axis(arg, axis, region)),
        _ => false,
    }
}

fn reads_local_axis(expr: &ExprSpec, axis: &str, region: &ValidatedRegion) -> bool {
    match expr {
        ExprSpec::Read {
            tensor, indices, ..
        } => {
            !region.program.inputs.contains(tensor)
                && indices.iter().any(
                    |index| matches!(index, super::syntax::IndexSpec::Axis(name) if name == axis),
                )
        }
        ExprSpec::Call { args, .. } => args.iter().any(|arg| reads_local_axis(arg, axis, region)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{AxisStep, IntervalProduct};

    #[test]
    fn scalar_iteration_yields_one_empty_interval_map() {
        let mut intervals = IntervalProduct::new(vec![]);
        assert!(intervals.next().unwrap().is_empty());
        assert!(intervals.next().is_none());
    }

    #[test]
    fn large_steps_clip_tails_and_finish_after_addition_overflows() {
        let mut intervals = IntervalProduct::new(vec![AxisStep {
            name: "i".into(),
            extent: usize::MAX,
            step: usize::MAX - 1,
        }]);
        assert_eq!(intervals.next().unwrap()["i"], 0..usize::MAX - 1);
        assert_eq!(intervals.next().unwrap()["i"], usize::MAX - 1..usize::MAX);
        assert!(intervals.next().is_none());
    }
}
