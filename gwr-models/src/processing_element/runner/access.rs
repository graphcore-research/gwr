// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Boundary access planning shared by execution and traffic estimation.

use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::types::SimError;

use super::{find_read_indices, plan_window_chunk_access, slice_axes, slice_view};
use crate::processing_element::program::executable::Expr;
use crate::processing_element::program::prepared::GroupPlan;
use crate::processing_element::program::syntax::{IndexSpec, ReductionOp};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ProgramFacts;
use crate::processing_element::tensors::TensorView;

/// Inputs loaded once and shared by the assignments in this tile.
pub(super) fn collect_initial_input_reads(plan: GroupPlan<'_>) -> Vec<InputRead<'_>> {
    plan.inputs()
        .filter(|(name, _)| !input_requires_assignment_reads(plan, name))
        .filter_map(|(name, view)| {
            plan.assignments()
                .find_map(|assignment| find_input_read(name, view, &assignment.expr))
        })
        .collect()
}

/// Reduction chunks and differing index mappings need their own input reads.
pub(super) fn collect_assignment_input_reads(
    plan: GroupPlan<'_>,
    assignment: usize,
) -> Vec<InputRead<'_>> {
    let expr = &plan.region().program.assignments[assignment].expr;
    let reduction = matches!(expr, Expr::Reduce { .. } | Expr::MatrixMac { .. });
    let mut reads = plan
        .inputs()
        .filter(|(name, _)| reduction || input_requires_assignment_reads(plan, name))
        .filter_map(|(name, view)| find_input_read(name, view, expr))
        .collect::<Vec<_>>();
    // Preserve the runner's stable issue order within an assignment.
    reads.sort_by_key(|read| read.name);
    reads
}

fn find_input_read<'a>(
    name: &'a str,
    view: &'a TensorView,
    expr: &'a Expr,
) -> Option<InputRead<'a>> {
    let indices = find_read_indices(expr, name)?;
    let window = matches!(
        expr,
        Expr::Reduce {
            op: ReductionOp::Min | ReductionOp::Max,
            ..
        }
    ) || indices
        .iter()
        .any(|index| matches!(index, IndexSpec::Affine { .. }));
    Some(InputRead {
        name,
        view,
        indices,
        window,
    })
}

pub(super) struct InputRead<'a> {
    pub name: &'a str,
    view: &'a TensorView,
    indices: &'a [IndexSpec],
    window: bool,
}

impl InputRead<'_> {
    pub fn plan_access(
        &self,
        tile: &RegionTile,
        region: &ProgramFacts,
        chunk: &BTreeMap<String, Range<usize>>,
    ) -> Result<InputAccess, SimError> {
        if self.window {
            let access = plan_window_chunk_access(self.view, self.indices, tile, region, chunk)?;
            Ok(InputAccess::Window {
                ranges: access.ranges,
                bytes: access.transfer_bytes,
            })
        } else {
            let mut intervals = tile.output_intervals.clone();
            intervals.extend(chunk.clone());
            slice_view(self.view, self.indices, &intervals).map(InputAccess::View)
        }
    }
}

pub(super) enum InputAccess {
    View(TensorView),
    Window {
        ranges: Vec<Range<u128>>,
        bytes: usize,
    },
}

impl InputAccess {
    pub fn bytes(&self) -> usize {
        match self {
            Self::View(view) => view.layout().num_access_bytes(),
            Self::Window { bytes, .. } => *bytes,
        }
    }

    pub fn ranges(&self) -> impl Iterator<Item = Range<u128>> + '_ {
        let (view, ranges): (Option<&TensorView>, &[Range<u128>]) = match self {
            Self::View(view) => (Some(view), &[]),
            Self::Window { ranges, .. } => (None, ranges),
        };
        view.into_iter()
            .flat_map(TensorView::address_ranges)
            .chain(ranges.iter().cloned())
    }
}

pub(super) fn build_tile_output_views(
    plan: GroupPlan<'_>,
    tile: &RegionTile,
) -> Result<Vec<TensorView>, SimError> {
    plan.outputs()
        .map(|(name, view)| {
            let assignment = &plan.region().program.assignments[plan.region().producers[name]];
            slice_axes(view, &assignment.axes, &tile.output_intervals)
        })
        .collect()
}

pub(super) fn input_requires_assignment_reads(plan: GroupPlan<'_>, name: &str) -> bool {
    let mappings = plan
        .assignments()
        .filter_map(|assignment| find_read_indices(&assignment.expr, name))
        .collect::<Vec<_>>();
    if mappings.windows(2).any(|pair| pair[0] != pair[1]) {
        return true;
    }
    plan.assignments().any(|assignment| match &assignment.expr {
        Expr::Reduce { .. } => find_read_indices(&assignment.expr, name).is_some(),
        Expr::MatrixMac { over, .. } => {
            !over.is_empty() && find_read_indices(&assignment.expr, name).is_some()
        }
        Expr::Scalar(_) => false,
    })
}
