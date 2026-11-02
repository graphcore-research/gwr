// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Lazy iteration of the tile intervals explicitly requested by a compute node.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use gwr_engine::sim_error;
use gwr_engine::types::{SimError, SimResult};

use super::schedule::plan_schedule;
use super::syntax::{ExprSpec, TileSpec};
use super::validate::ValidatedRegion;

/// Validate either the flat tile plan or inferred phased execution.
pub fn validate_tiling(region: &ValidatedRegion, tiling: Option<&TileSpec>) -> SimResult {
    if plan_schedule(region, tiling)?.is_none() {
        plan_tiles(region, tiling)?;
    }
    Ok(())
}

pub(in crate::processing_element) fn plan_tiles(
    region: &ValidatedRegion,
    tiling: Option<&TileSpec>,
) -> Result<TileSchedule, SimError> {
    if plan_schedule(region, tiling)?.is_some() {
        return sim_error!("inferred phased schedule requires scoped execution, not plan_tiles");
    }
    let mut parallel = Vec::new();
    let mut seen_parallel = BTreeSet::new();
    let mut reduction = BTreeSet::new();
    for assignment in &region.program.assignments {
        for axis in &assignment.axes {
            if seen_parallel.insert(axis.clone()) {
                parallel.push(axis.clone());
            }
        }
        collect_reductions(&assignment.expr, &mut reduction);
    }
    let default_sizes = BTreeMap::new();
    let sizes = tiling.map_or(&default_sizes, |spec| &spec.sizes);
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
pub(in crate::processing_element) struct TileSchedule {
    outputs: Vec<AxisStep>,
    reductions: Vec<AxisStep>,
}

impl TileSchedule {
    pub(in crate::processing_element) fn tiles(&self) -> impl Iterator<Item = RegionTile> + '_ {
        IntervalProduct::new(self.outputs.clone()).map(|output_intervals| RegionTile {
            output_intervals,
            reductions: self.reductions.clone(),
        })
    }
}

#[derive(Clone, Debug)]
pub(in crate::processing_element) struct RegionTile {
    pub(in crate::processing_element) output_intervals: BTreeMap<String, Range<usize>>,
    reductions: Vec<AxisStep>,
}

impl RegionTile {
    pub(in crate::processing_element) fn new(
        output_intervals: BTreeMap<String, Range<usize>>,
        reductions: BTreeMap<String, usize>,
        region: &ValidatedRegion,
    ) -> Result<Self, SimError> {
        for (axis, interval) in &output_intervals {
            let extent = region
                .extent(axis)
                .ok_or_else(|| SimError(format!("unknown tile axis {axis}")))?;
            if interval.start >= interval.end || interval.end > extent {
                return sim_error!("invalid tile interval for {axis}");
            }
        }
        let reductions = reductions
            .into_iter()
            .map(|(name, step)| {
                let extent = region
                    .extent(&name)
                    .ok_or_else(|| SimError(format!("unknown reduction axis {name}")))?;
                if step == 0 {
                    return sim_error!("reduction tile size for {name} must be positive");
                }
                Ok(AxisStep { name, extent, step })
            })
            .collect::<Result<Vec<_>, SimError>>()?;
        Ok(Self {
            output_intervals,
            reductions,
        })
    }

    #[cfg(test)]
    pub(in crate::processing_element) fn reduction_chunks(
        &self,
    ) -> impl Iterator<Item = BTreeMap<String, Range<usize>>> + '_ {
        IntervalProduct::new(self.reductions.clone())
    }

    pub(in crate::processing_element) fn reduction_chunks_for(
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

    pub(in crate::processing_element) fn reduction_chunk_count_for(
        &self,
        axes: &[String],
    ) -> Result<usize, SimError> {
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
    for expr in expr.walk() {
        if let ExprSpec::Reduce { over, .. } | ExprSpec::MatrixMac { over, .. } = expr {
            axes.extend(over.iter().cloned());
        }
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
