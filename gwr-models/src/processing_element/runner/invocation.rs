// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Pure, lazy invocation arithmetic shared by preparation and execution.
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use gwr_engine::types::SimError;

use super::helpers::{build_scalar_expression_work, find_first_read_dtype};
use super::matrix_shape::build_matrix_work;
use super::window::{count_window_chunk_operations, window_indices_are_in_range};
use crate::processing_element::compute::ComputeWork;
use crate::processing_element::program::executable::{Assignment, Expr, Read, ScalarExpr};
use crate::processing_element::program::syntax::{BoundsMode, IndexSpec, ReductionOp};
use crate::processing_element::program::tile::{IntervalProduct, RegionTile};
use crate::processing_element::program::validate::ProgramFacts;
use crate::processing_element::tensors::TensorView;

pub(super) struct Invocation {
    pub chunk: BTreeMap<String, Range<usize>>,
    pub work: ComputeWork,
    pub read_label: Option<String>,
}

pub(super) struct AssignmentInvocations<'a> {
    region: &'a ProgramFacts,
    assignment: &'a Assignment,
    tile: &'a RegionTile,
    chunks: IntervalProduct,
    chunk_index: usize,
    outputs: usize,
    excluded: Vec<(&'a [IndexSpec], &'a TensorView)>,
    in_range: bool,
    seen: BTreeSet<Vec<usize>>,
    finished: bool,
}

impl<'a> AssignmentInvocations<'a> {
    pub fn new(
        region: &'a ProgramFacts,
        index: usize,
        tile: &'a RegionTile,
    ) -> Result<Self, SimError> {
        let assignment = &region.program.assignments[index];
        let over = reduction_axes(&assignment.expr);
        let outputs = assignment.axes.iter().try_fold(1usize, |count, axis| {
            count
                .checked_mul(
                    tile.output_intervals
                        .get(axis)
                        .map_or(region.extents[axis], Range::len),
                )
                .ok_or_else(|| SimError("tile output count overflows".into()))
        })?;
        let excluded = assignment
            .expr
            .iter_reads()
            .filter_map(|read| {
                if read.bounds != BoundsMode::Exclude {
                    return None;
                }
                region
                    .program
                    .inputs
                    .iter()
                    .position(|name| name == &read.tensor)
                    .map(|i| (read.indices.as_slice(), &region.bindings.inputs[i]))
            })
            .collect::<Vec<_>>();
        let in_range = excluded.iter().try_fold(true, |valid, (indices, input)| {
            Ok::<_, SimError>(valid && window_indices_are_in_range(indices, tile, region, input)?)
        })?;
        Ok(Self {
            region,
            assignment,
            tile,
            chunks: tile.reduction_chunks_for(over),
            chunk_index: 0,
            outputs,
            excluded,
            in_range,
            seen: BTreeSet::new(),
            finished: false,
        })
    }

    fn chunk(&mut self, chunk: BTreeMap<String, Range<usize>>) -> Result<Invocation, SimError> {
        let assignment = self.assignment;
        let expr = &assignment.expr;
        let output_dtype = self
            .region
            .value(assignment.primary_result())
            .unwrap()
            .dtype;
        let index = self.chunk_index;
        self.chunk_index += 1;
        let mut intervals = self.tile.output_intervals.clone();
        intervals.extend(chunk.clone());
        let (work, label) = match expr {
            Expr::Scalar(value) => (
                build_scalar_expression_work(value, self.region, output_dtype, self.outputs)?.0,
                "phase reread".to_owned(),
            ),
            Expr::MatrixMac { .. } => (
                build_matrix_work(assignment, self.region, &intervals)?.expect("matrix geometry"),
                format!("chunk {index} read"),
            ),
            Expr::Reduce { op, over, value } => {
                let (elements, reductions) = count_window_chunk_operations(
                    &self.excluded,
                    &assignment.axes,
                    over,
                    self.tile,
                    self.region,
                    &chunk,
                    index == 0,
                    self.in_range,
                    &mut self.seen,
                )?;
                let dtype = match (value, op) {
                    (
                        ScalarExpr::Read(Read { tensor, .. }),
                        ReductionOp::Min | ReductionOp::Max,
                    ) => self.region.value(tensor).unwrap().dtype,
                    (_, ReductionOp::Min | ReductionOp::Max) if assignment.output().is_none() => {
                        find_first_read_dtype(value, self.region)
                    }
                    _ => output_dtype,
                };
                let mut work = build_scalar_expression_work(value, self.region, dtype, elements)?.0;
                if reductions != 0 {
                    let operation = if *op == ReductionOp::Sum {
                        "add"
                    } else {
                        "compare"
                    };
                    work.stages.extend(
                        ComputeWork::operation(operation, vec![dtype, dtype], reductions).stages,
                    );
                }
                let label = match op {
                    ReductionOp::Sum => format!("chunk {index} read"),
                    _ if matches!(value, ScalarExpr::Call { .. }) => {
                        format!("extremum chunk {index} read")
                    }
                    _ => format!("window chunk {index} read"),
                };
                (work, label)
            }
        };
        Ok(Invocation {
            chunk,
            work,
            read_label: Some(label),
        })
    }
}

impl Iterator for AssignmentInvocations<'_> {
    type Item = Result<Invocation, SimError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        if let Some(chunk) = self.chunks.next() {
            return Some(self.chunk(chunk));
        }
        self.finished = true;
        match &self.assignment.expr {
            Expr::Reduce {
                op: ReductionOp::Min | ReductionOp::Max,
                ..
            } if !self.in_range && self.seen.len() != self.outputs => {
                Some(Err(SimError("empty min/max window".into())))
            }
            Expr::MatrixMac { accumulator, .. }
                if *accumulator
                    != self
                        .region
                        .value(self.assignment.primary_result())
                        .unwrap()
                        .dtype =>
            {
                Some(Ok(Invocation {
                    chunk: BTreeMap::new(),
                    work: ComputeWork::operation("convert", vec![*accumulator], self.outputs),
                    read_label: None,
                }))
            }
            _ => None,
        }
    }
}

fn reduction_axes(expr: &Expr) -> &[String] {
    expr.reduction_axes()
}
