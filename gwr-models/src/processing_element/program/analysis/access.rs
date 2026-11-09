// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Boundary access planning shared by execution and traffic estimation.

use std::collections::BTreeMap;
use std::ops::Range;

use gwr_engine::types::SimError;

use super::helpers::{slice_axes, slice_view};
use super::window::plan_window_chunk_access;
use crate::processing_element::program::executable::Expr;
use crate::processing_element::program::prepared::GroupPlan;
use crate::processing_element::program::syntax::{IndexSpec, ReductionOp};
use crate::processing_element::program::tile::RegionTile;
use crate::processing_element::program::validate::ProgramFacts;
use crate::processing_element::tensors::TensorView;

/// Owned read locations refer to the original bound grammar, not cloned syntax.
#[derive(Clone, Copy, Debug)]
pub(in crate::processing_element) struct InputReadPlan {
    pub input: usize,
    pub(super) assignment: usize,
    read: usize,
    window: bool,
}

#[derive(Debug)]
pub(in crate::processing_element) struct GroupInputs {
    pub initial: Vec<InputReadPlan>,
    pub per_assignment: BTreeMap<usize, Vec<InputReadPlan>>,
    pub buffers: Vec<InputBufferPlan>,
}

#[derive(Debug)]
pub(in crate::processing_element) struct InputBufferPlan {
    pub read: InputReadPlan,
    pub per_assignment: bool,
}

impl GroupInputs {
    pub fn new(region: &ProgramFacts, assignments: &[usize]) -> Self {
        let mut initial = Vec::new();
        let mut buffers = Vec::new();
        let mut per_assignment = assignments
            .iter()
            .map(|&i| (i, Vec::new()))
            .collect::<BTreeMap<_, _>>();
        // Initial loads and buffers retain boundary input declaration order.
        for input in 0..region.program.inputs.len() {
            let reads = assignments
                .iter()
                .filter_map(|&assignment| InputReadPlan::new(region, input, assignment))
                .collect::<Vec<_>>();
            let Some(&first) = reads.first() else {
                continue;
            };
            let reread = reads
                .windows(2)
                .any(|pair| pair[0].resolve(region).indices != pair[1].resolve(region).indices)
                || reads.iter().any(|read| {
                    match &region.program.assignments[read.assignment].expr {
                        Expr::Reduce { .. } => true,
                        Expr::MatrixMac { over, .. } => !over.is_empty(),
                        Expr::Scalar(_) => false,
                    }
                });
            if !reread {
                initial.push(first);
            }
            buffers.push(InputBufferPlan {
                read: first,
                per_assignment: reread,
            });
            for read in reads {
                let expr = &region.program.assignments[read.assignment].expr;
                if reread || matches!(expr, Expr::Reduce { .. } | Expr::MatrixMac { .. }) {
                    per_assignment.get_mut(&read.assignment).unwrap().push(read);
                }
            }
        }
        // Issue assignment reads in input-name order.
        for reads in per_assignment.values_mut() {
            reads.sort_by_key(|read| &region.program.inputs[read.input]);
        }
        Self {
            initial,
            per_assignment,
            buffers,
        }
    }
}

/// Inputs loaded once and shared by the assignments in this tile.
pub(in crate::processing_element) fn collect_initial_input_reads(
    plan: GroupPlan<'_>,
) -> impl Iterator<Item = InputRead<'_>> {
    plan.group
        .inputs
        .initial
        .iter()
        .map(move |read| read.resolve(plan.region()))
}

/// Resolve the prepared read descriptors once for this assignment execution.
pub(in crate::processing_element) fn collect_assignment_input_reads(
    plan: GroupPlan<'_>,
    assignment: usize,
) -> Vec<InputRead<'_>> {
    plan.group.inputs.per_assignment[&assignment]
        .iter()
        .map(|read| read.resolve(plan.region()))
        .collect()
}

impl InputReadPlan {
    fn new(region: &ProgramFacts, input: usize, assignment: usize) -> Option<Self> {
        let expr = &region.program.assignments[assignment].expr;
        let (read, source) = expr
            .iter_reads()
            .enumerate()
            .find(|(_, read)| read.tensor == region.program.inputs[input])?;
        let window = matches!(
            expr,
            Expr::Reduce {
                op: ReductionOp::Min | ReductionOp::Max,
                ..
            }
        ) || source
            .indices
            .iter()
            .any(|index| matches!(index, IndexSpec::Affine { .. }));
        Some(Self {
            input,
            assignment,
            read,
            window,
        })
    }

    pub fn resolve(self, region: &ProgramFacts) -> InputRead<'_> {
        let source = region.program.assignments[self.assignment]
            .expr
            .iter_reads()
            .nth(self.read)
            .expect("prepared read location");
        InputRead {
            name: &region.program.inputs[self.input],
            view: &region.bindings.inputs[self.input],
            indices: &source.indices,
            window: self.window,
        }
    }
}

pub(in crate::processing_element) struct InputRead<'a> {
    pub name: &'a str,
    pub(super) view: &'a TensorView,
    pub(super) indices: &'a [IndexSpec],
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

pub(in crate::processing_element) enum InputAccess {
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

pub(in crate::processing_element) fn build_tile_output_views<'a>(
    plan: GroupPlan<'a>,
    tile: &RegionTile,
) -> Result<Vec<(&'a str, TensorView)>, SimError> {
    plan.group
        .outputs
        .iter()
        .map(|output| {
            let region = plan.region();
            let name = &region.program.outputs[output.binding];
            let view = &region.bindings.outputs[output.binding];
            let assignment = &region.program.assignments[output.producer];
            Ok((
                name.as_str(),
                slice_axes(view, &assignment.axes, &tile.output_intervals)?,
            ))
        })
        .collect()
}
