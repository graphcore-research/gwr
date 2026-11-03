// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Boundary access planning shared by execution and traffic estimation.

use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::types::SimError;

use super::{find_read_indices, plan_window_chunk_access_ranges, slice_axes, slice_view};
use crate::processing_element::program::syntax::{ExprSpec, IndexSpec, ReductionOp};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ValidatedRegion;
use crate::processing_element::task::ProgramTaskConfig;
use crate::processing_element::tensors::TensorView;

/// Inputs loaded once and shared by the assignments in this tile.
pub(super) fn collect_initial_input_reads(config: &ProgramTaskConfig) -> Vec<InputRead<'_>> {
    config
        .program
        .inputs
        .iter()
        .zip(&config.inputs)
        .filter(|(name, _)| !input_requires_assignment_reads(config, name))
        .filter_map(|(name, view)| {
            config
                .program
                .assignments
                .iter()
                .find_map(|assignment| find_input_read(name, view, &assignment.expr))
        })
        .collect()
}

/// Reduction chunks and differing index mappings need their own input reads.
pub(super) fn collect_assignment_input_reads(
    config: &ProgramTaskConfig,
    assignment: usize,
) -> Vec<InputRead<'_>> {
    let expr = &config.program.assignments[assignment].expr;
    let reduction = matches!(expr, ExprSpec::Reduce { .. } | ExprSpec::MatrixMac { .. });
    let mut reads = config
        .program
        .inputs
        .iter()
        .zip(&config.inputs)
        .filter(|(name, _)| reduction || input_requires_assignment_reads(config, name))
        .filter_map(|(name, view)| find_input_read(name, view, expr))
        .collect::<Vec<_>>();
    // Preserve the runner's stable issue order within an assignment.
    reads.sort_by_key(|read| read.name);
    reads
}

fn find_input_read<'a>(
    name: &'a str,
    view: &'a TensorView,
    expr: &'a ExprSpec,
) -> Option<InputRead<'a>> {
    let indices = find_read_indices(expr, name)?;
    let window = matches!(
        expr,
        ExprSpec::Reduce {
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
        region: &ValidatedRegion,
        chunk: &BTreeMap<String, Range<usize>>,
    ) -> Result<InputAccess, SimError> {
        if self.window {
            let (ranges, bytes) =
                plan_window_chunk_access_ranges(self.view, self.indices, tile, region, chunk)?;
            Ok(InputAccess::Window { ranges, bytes })
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
    config: &ProgramTaskConfig,
    tile: &RegionTile,
) -> Result<Vec<TensorView>, SimError> {
    config
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
        .collect()
}

pub(super) fn input_requires_assignment_reads(config: &ProgramTaskConfig, name: &str) -> bool {
    let mappings = config
        .program
        .assignments
        .iter()
        .filter_map(|assignment| find_read_indices(&assignment.expr, name))
        .collect::<Vec<_>>();
    if mappings.windows(2).any(|pair| pair[0] != pair[1]) {
        return true;
    }
    config
        .program
        .assignments
        .iter()
        .any(|assignment| match &assignment.expr {
            ExprSpec::Reduce { value, .. } => find_read_indices(value, name).is_some(),
            ExprSpec::MatrixMac { over, .. } => {
                !over.is_empty() && find_read_indices(&assignment.expr, name).is_some()
            }
            _ => false,
        })
}
