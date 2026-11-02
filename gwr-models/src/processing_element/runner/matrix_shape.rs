// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Logical matrix geometry shared by compute validation and execution chunks.
//!
//! Matrix execution units are assumed to consume either operand, or both
//! operands, in transposed matrix-axis order without additional conversion work
//! or SRAM. Geometry ignores stored axis order; memory access planning retains
//! it.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use gwr_engine::types::SimError;

use crate::processing_element::compute::{ComputeWork, MatrixWorkShape};
use crate::processing_element::program::syntax::{AssignmentSpec, BoundsMode, ExprSpec, IndexSpec};
use crate::processing_element::program::validate::ValidatedRegion;
use crate::processing_element::tensors::dtype::DataType;

pub(super) fn build_matrix_work(
    assignment: &AssignmentSpec,
    region: &ValidatedRegion,
    intervals: &BTreeMap<String, Range<usize>>,
) -> Result<Option<ComputeWork>, SimError> {
    let ExprSpec::MatrixMac {
        args,
        over,
        accumulator,
        ..
    } = &assignment.expr
    else {
        return Ok(None);
    };
    if args.len() != 2 || over.len() != 1 {
        return Err(SimError(
            "matrix geometry requires two operands and one contraction axis".into(),
        ));
    }
    let mut inputs = Vec::new();
    let mut operand_axes = Vec::new();
    for arg in args {
        let (dtype, axes) = resolve_matrix_operand(arg, region, &over[0])?;
        inputs.push(dtype);
        operand_axes.push(axes);
    }
    let shape =
        derive_matrix_work_shape(&assignment.axes, &operand_axes, &over[0], region, intervals)?;
    Ok(Some(ComputeWork::matrix_mac(inputs, *accumulator, shape)?))
}

fn derive_matrix_work_shape(
    output_axes: &[String],
    operand_axes: &[BTreeSet<&str>],
    contraction_axis: &str,
    region: &ValidatedRegion,
    intervals: &BTreeMap<String, Range<usize>>,
) -> Result<MatrixWorkShape, SimError> {
    let mut shape = MatrixWorkShape {
        batches: 1,
        m: 1,
        n: 1,
        k: resolve_matrix_axis_length(contraction_axis, region, intervals)?,
    };
    for axis in output_axes {
        // Singleton dimensions still broadcast along their named logical axis.
        // Axes shared by both operands select independent contractions. Axes
        // absent from both repeat the same contraction in the output.
        let dimension = match (
            operand_axes[0].contains(axis.as_str()),
            operand_axes[1].contains(axis.as_str()),
        ) {
            (true, false) => &mut shape.m,
            (false, true) => &mut shape.n,
            _ => &mut shape.batches,
        };
        *dimension = dimension
            .checked_mul(resolve_matrix_axis_length(axis, region, intervals)?)
            .ok_or_else(|| SimError("matrix shape dimension product overflows".into()))?;
    }
    Ok(shape)
}

fn resolve_matrix_operand<'a>(
    arg: &'a ExprSpec,
    region: &ValidatedRegion,
    contraction_axis: &str,
) -> Result<(DataType, BTreeSet<&'a str>), SimError> {
    let ExprSpec::Read {
        tensor,
        indices,
        bounds,
    } = arg
    else {
        return Err(SimError("matrix operand must be a direct read".into()));
    };
    if *bounds != BoundsMode::RequireInRange {
        return Err(SimError("matrix operands require in-range indexing".into()));
    }
    let dtype = region
        .value(tensor)
        .ok_or_else(|| SimError(format!("unknown matrix operand {tensor}")))?
        .dtype;
    let axes = indices
        .iter()
        .filter_map(|index| match index {
            IndexSpec::Axis(axis) => Some(axis.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if indices
        .iter()
        .any(|index| matches!(index, IndexSpec::Affine { .. }))
        || !axes.contains(contraction_axis)
    {
        return Err(SimError(
            "matrix operands must directly index the contraction axis".into(),
        ));
    }
    Ok((dtype, axes))
}

fn resolve_matrix_axis_length(
    axis: &str,
    region: &ValidatedRegion,
    intervals: &BTreeMap<String, Range<usize>>,
) -> Result<usize, SimError> {
    let extent = region
        .extent(axis)
        .ok_or_else(|| SimError(format!("unknown matrix axis {axis}")))?;
    let interval = intervals.get(axis).cloned().unwrap_or(0..extent);
    if interval.start >= interval.end || interval.end > extent {
        return Err(SimError(
            "matrix interval is outside resolved extent".into(),
        ));
    }
    Ok(interval.len())
}
